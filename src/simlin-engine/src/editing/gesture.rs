// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Gesture planning: what a tap does, and what each frame of a drag previews
//! and commits.
//!
//! The host owns input policy -- telling a tap from a drag from a pinch,
//! double taps, keyboard modifiers, and the tool its toolbar arms -- and hands
//! the planner resolved presses in model coordinates. A tap is planned once
//! (`plan_tap`). A drag begins a `GestureSession` (`begin_drag`), which
//! classifies the press against the base view and then plans frames. Every
//! frame evaluates the pointer against that base view, so the commit is the
//! frame at the release point and preview cannot diverge from commit (E2), and
//! a frame changes only what the gesture touches (E4). There is no click
//! threshold inside a drag: the host starts one only past its own slop, and a
//! pointer back at the press previews exactly the base geometry, which then
//! commits nothing. The session holds the one decision a function of the
//! pointer could not keep once the pointer comes back: which way a press on a
//! pipe latched, sliding the valve or offsetting the segment, decided by the
//! first movement. A move of the selection by an offset (`plan_move`) is the
//! move-selection frame at that offset, which a host plans without a session to
//! nudge the selection from the keyboard.
//!
//! Flow geometry comes only from the core (`route`, `route_end`,
//! `offset_segment`, `slide_valve`, `heal`). A drop is valid exactly when the
//! routed flow holds G2-G6 and the semantic rules hold (a flow's ends on
//! different stocks, a stock whose variable exists); an invalid drop commits
//! nothing (E6). Loose imported flow ends get clouds when an edit routes them,
//! and links whose endpoints moved follow once, from the final elements.
//!
//! Every edit is planned through one rule (`Changes::into_plan`): a plan whose
//! changes leave every element as the base holds it plans nothing, so a frame,
//! its release and a nudge agree that it lands nothing, and a plan holding a
//! non-finite number commits nothing: the scene draws nothing for a part holding
//! one, so landing it would make what the edit moved vanish. A drop target is
//! valid exactly where the drop is allowed (`Changes::into_drop`).

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{
    Aux, Cloud, Flow, FlowPoint, LabelSide, Link, LinkShape, Module, Stock,
};
use crate::diagram::constants::AUX_RADIUS;

use super::base::{
    BaseView, VariableKind, is_finite, is_link_source, label_side_of, position_of, translated,
    with_label_side,
};
use super::geometry::{Axis, FlowEnd, GEOMETRY_EPSILON, Point};
use super::heal::heal;
use super::hit::{Hit, HitPart};
use super::links::{
    follow_links, label_side_for_pointer, shape_through, takeoff_through, visual_center,
};
use super::offset::{offset_segment, segment_hold};
use super::path::{slide_valve, translate};
use super::route::{route, route_end};
use super::terminal::{
    CloudMoves, CloudRef, Terminal, Terminals, flow_terminals, free_terminal, path_of,
    stock_terminal, target_stock_terminal,
};
use super::validity::{Fault, flow_fault};

/// The tool a host's toolbar arms.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Stock,
    Flow,
    Aux,
    Link,
    Module,
}

impl Tool {
    pub const ALL: [Tool; 5] = [Tool::Stock, Tool::Flow, Tool::Aux, Tool::Link, Tool::Module];
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Touch,
    Pencil,
    Mouse,
}

/// A press, resolved by the host.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Press {
    /// Model coordinates.
    pub point: Point,
    /// What the press landed on (`hit_test`), `None` for the empty canvas.
    pub hit: Option<Hit>,
    pub tool: Option<Tool>,
    /// The selection before the press.
    pub selection: Vec<i32>,
    /// A selection modifier (Shift or Command) is held: a tap toggles the hit
    /// element, and a drag adds it to what moves.
    pub toggle: bool,
    pub pointer: PointerKind,
    /// How far outside a drop target the pointer may be and still land on it,
    /// in model units.
    pub target_slop: f64,
}

/// What a plan asks the host to do once the gesture ends.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitKind {
    /// Nothing: an idle frame, an invalid drop, or an edit holding a number the
    /// scene cannot draw.
    None,
    /// Apply the plan's edit (`Plan::edit`) and adopt its selection.
    Edit,
    /// Adopt the plan's selection; nothing in the view changes.
    Select,
}

/// A drop target under the pointer, drawn highlighted when valid and as a
/// refusal when not. Validity is whether the drop is allowed: a drop that
/// changes nothing (an end dropped back where it was) is allowed and commits
/// nothing, and a drop whose result holds a number the scene cannot draw is
/// not.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub struct Target {
    pub uid: i32,
    pub valid: bool,
}

/// A link being drawn or reattached while its end is over no valid target: the
/// preview draws it straight from `from` to the pointer, under `uid` (the
/// reattached link's own, or the uid a created link would take).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub struct DanglingLink {
    pub uid: i32,
    pub from: i32,
    pub to: Point,
}

/// An edit of one view: elements to upsert by uid (substituting or adding), and
/// uids to remove. `ModelOperation::EditView` derives the model operations the
/// edit implies when it applies.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default)]
pub struct ViewEdit {
    pub upsert: Vec<ViewElement>,
    pub remove: Vec<i32>,
}

impl ViewEdit {
    pub fn is_empty(&self) -> bool {
        self.upsert.is_empty() && self.remove.is_empty()
    }
}

/// One frame of a gesture, or a tap.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Plan {
    /// Elements the frame draws in place of the base elements with the same uid,
    /// or adds, each differing from what the base holds: substitutions in view
    /// order, then additions in creation order.
    pub changed: Vec<ViewElement>,
    /// Base elements the frame does not draw.
    pub removed: Vec<i32>,
    pub target: Option<Target>,
    pub commit: CommitKind,
    /// The selection the preview draws and the end of the gesture adopts.
    pub selection: Vec<i32>,
    /// An element whose name editor opens once the edit lands.
    pub handoff: Option<i32>,
    /// A tap on an element's body, which opens its details.
    pub details: bool,
    pub dangling_link: Option<DanglingLink>,
    /// The edit's name in errors and history; empty unless `commit` is `Edit`.
    pub label: &'static str,
}

impl Plan {
    fn idle(selection: Vec<i32>) -> Plan {
        Plan {
            changed: Vec::new(),
            removed: Vec::new(),
            target: None,
            commit: CommitKind::None,
            selection,
            handoff: None,
            details: false,
            dangling_link: None,
            label: "",
        }
    }

    fn select(current: &[i32], selection: Vec<i32>) -> Plan {
        let same =
            current.len() == selection.len() && current.iter().all(|u| selection.contains(u));
        Plan {
            commit: if same {
                CommitKind::None
            } else {
                CommitKind::Select
            },
            ..Plan::idle(selection)
        }
    }

    /// The view edit an `Edit` commit applies: the changed elements and the
    /// removals, never both empty (`Changes::into_plan`). `None` for any other
    /// commit.
    pub fn edit(&self) -> Option<ViewEdit> {
        (self.commit == CommitKind::Edit).then(|| ViewEdit {
            upsert: self.changed.clone(),
            remove: self.removed.clone(),
        })
    }
}

/// The elements a frame changes, by uid, and the base elements it removes.
#[derive(Default)]
struct Changes {
    elements: HashMap<i32, ViewElement>,
    /// Uids the base lacks, in creation order.
    added: Vec<i32>,
    removed: Vec<i32>,
}

impl Changes {
    fn set(&mut self, base: &BaseView, element: ViewElement) {
        let uid = element.get_uid();
        if base.get(uid).is_none() && !self.elements.contains_key(&uid) {
            self.added.push(uid);
        }
        self.elements.insert(uid, element);
    }

    fn get<'a>(&'a self, base: &'a BaseView, uid: i32) -> Option<&'a ViewElement> {
        self.elements.get(&uid).or_else(|| base.get(uid))
    }

    fn remove(&mut self, uid: i32) {
        self.elements.remove(&uid);
        self.added.retain(|u| *u != uid);
        if !self.removed.contains(&uid) {
            self.removed.push(uid);
        }
    }

    fn move_clouds(&mut self, base: &BaseView, moves: &CloudMoves) {
        for m in moves {
            if let Some(ViewElement::Cloud(cloud)) = self.get(base, m.uid) {
                let mut cloud = cloud.clone();
                cloud.x = m.at.x;
                cloud.y = m.at.y;
                self.set(base, ViewElement::Cloud(cloud));
            }
        }
    }

    /// The plan of an edit: the changes plus every link that follows a moved
    /// endpoint, less what equals the base (`Changes::settle`).
    fn into_plan(self, base: &BaseView, selection: Vec<i32>, label: &'static str) -> Plan {
        self.settle(base, selection, label).0
    }

    /// The plan of a drop onto `target`, which the gesture allows: the target is
    /// marked valid unless the plan holds a number the scene cannot draw, which
    /// refuses the drop. A drop that changes nothing stays allowed and plans
    /// nothing.
    fn into_drop(
        self,
        base: &BaseView,
        selection: Vec<i32>,
        label: &'static str,
        target: i32,
    ) -> Plan {
        let (plan, refused) = self.settle(base, selection, label);
        Plan {
            target: Some(Target {
                uid: target,
                valid: !refused,
            }),
            ..plan
        }
    }

    /// The plan of the changes, and whether it refuses them. A plan changing
    /// nothing is idle, so a frame, its release and a nudge that move nothing
    /// all plan no edit. A plan holding a non-finite number (a coordinate an
    /// offset overflowed, a valve placed along a route longer than the largest
    /// coordinate) is refused and commits nothing: the scene draws nothing for a
    /// part holding one, so landing it would make what the edit moved vanish.
    fn settle(mut self, base: &BaseView, selection: Vec<i32>, label: &'static str) -> (Plan, bool) {
        for link in follow_links(base, &self.elements) {
            self.set(base, ViewElement::Link(link));
        }
        self.elements
            .retain(|&uid, element| base.get(uid) != Some(&*element));
        if self.elements.is_empty() && self.removed.is_empty() {
            return (Plan::idle(selection), false);
        }
        let refused = !self.elements.values().all(is_finite);
        let (changed, removed) = self.into_elements(base);
        let plan = Plan {
            changed,
            removed,
            commit: if refused {
                CommitKind::None
            } else {
                CommitKind::Edit
            },
            label: if refused { "" } else { label },
            ..Plan::idle(selection)
        };
        (plan, refused)
    }

    fn into_elements(mut self, base: &BaseView) -> (Vec<ViewElement>, Vec<i32>) {
        let mut substituted: Vec<(usize, i32)> = self
            .elements
            .keys()
            .filter_map(|&uid| base.index_of(uid).map(|i| (i, uid)))
            .collect();
        substituted.sort_unstable();
        let mut out = Vec::with_capacity(self.elements.len());
        for (_, uid) in substituted {
            out.extend(self.elements.remove(&uid));
        }
        for uid in &self.added {
            out.extend(self.elements.remove(uid));
        }
        (out, self.removed)
    }
}

fn point_of(p: &FlowPoint) -> Point {
    Point::new(p.x, p.y)
}

/// The index of the segment of `points` nearest `p`, the earliest on a tie.
fn segment_nearest(points: &[FlowPoint], p: Point) -> usize {
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    for (i, w) in points.windows(2).enumerate() {
        let (a, b) = (point_of(&w[0]), point_of(&w[1]));
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let l2 = dx * dx + dy * dy;
        let t = if l2 == 0.0 {
            0.0
        } else {
            (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
        };
        let d = (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy));
        if d < best_distance - GEOMETRY_EPSILON {
            best_distance = d;
            best = i;
        }
    }
    best
}

/// A flow as an edit routes it, and the clouds created or moved on the way.
struct Healed {
    flow: Flow,
    clouds: Vec<ViewElement>,
}

impl Healed {
    fn get<'a>(&'a self, base: &'a BaseView, uid: i32) -> Option<&'a ViewElement> {
        self.clouds
            .iter()
            .find(|c| c.get_uid() == uid)
            .or_else(|| base.get(uid))
    }

    fn terminals(&self, base: &BaseView) -> Terminals {
        flow_terminals(&self.flow, |uid| self.get(base, uid))
    }
}

/// `flow` with each end attached the way strict G1 requires -- an end with no
/// attachment, a dangling one, or one attached to anything but a stock or this
/// flow's own cloud gets a new cloud at its endpoint, taking uids from
/// `next_uid` -- and then healed against its terminals, the identity on a valid
/// attached flow.
fn healed(base: &BaseView, flow: &Flow, next_uid: &mut i32) -> Healed {
    let mut f = flow.clone();
    let mut clouds = Vec::new();
    let last = f.points.len() - 1;
    for index in [0, last] {
        let attached = match f.points[index]
            .attached_to_uid
            .and_then(|uid| base.get(uid))
        {
            Some(ViewElement::Stock(_)) => true,
            Some(ViewElement::Cloud(c)) => c.flow_uid == f.uid,
            _ => false,
        };
        if !attached {
            let uid = *next_uid;
            *next_uid += 1;
            let p = &f.points[index];
            clouds.push(ViewElement::Cloud(Cloud {
                uid,
                flow_uid: f.uid,
                x: p.x,
                y: p.y,
                compat: None,
            }));
            f.points[index].attached_to_uid = Some(uid);
        }
    }
    let mut h = Healed { flow: f, clouds };
    let g = heal(&h.flow, &h.terminals(base), base.stock_centers());
    for m in &g.clouds {
        match h.clouds.iter_mut().find(|c| c.get_uid() == m.uid) {
            Some(ViewElement::Cloud(c)) => {
                c.x = m.at.x;
                c.y = m.at.y;
            }
            _ => {
                if let Some(ViewElement::Cloud(c)) = base.get(m.uid) {
                    h.clouds.push(ViewElement::Cloud(Cloud {
                        x: m.at.x,
                        y: m.at.y,
                        ..c.clone()
                    }));
                }
            }
        }
    }
    h.flow = g.flow;
    h
}

fn stock_uids_of(terminals: &[Terminal]) -> SmallVec<[i32; 2]> {
    terminals
        .iter()
        .filter_map(|t| match t {
            Terminal::Stock { uid, .. } => Some(*uid),
            Terminal::Free { .. } => None,
        })
        .collect()
}

fn with_end(end: FlowEnd, terminal: Terminal, fixed: Terminal) -> Terminals {
    match end {
        FlowEnd::Source => Terminals {
            source: terminal,
            sink: fixed,
        },
        FlowEnd::Sink => Terminals {
            source: fixed,
            sink: terminal,
        },
    }
}

fn endpoint(flow: &Flow, end: FlowEnd) -> Point {
    match end {
        FlowEnd::Source => point_of(&flow.points[0]),
        FlowEnd::Sink => point_of(&flow.points[flow.points.len() - 1]),
    }
}

/// The element a creation tool places at `at`, under a default name.
fn created_element(base: &BaseView, tool: Tool, at: Point) -> Option<ViewElement> {
    let uid = base.next_uid();
    Some(match tool {
        Tool::Aux => ViewElement::Aux(Aux {
            name: base.allocate_name("New Variable"),
            uid,
            x: at.x,
            y: at.y,
            label_side: LabelSide::Right,
            compat: None,
        }),
        Tool::Stock => ViewElement::Stock(Stock {
            name: base.allocate_name("New Stock"),
            uid,
            x: at.x,
            y: at.y,
            label_side: LabelSide::Bottom,
            compat: None,
        }),
        Tool::Module => ViewElement::Module(Module {
            name: base.allocate_name("New Module"),
            uid,
            x: at.x,
            y: at.y,
            label_side: LabelSide::Bottom,
        }),
        Tool::Flow | Tool::Link => return None,
    })
}

fn create_element_plan(base: &BaseView, tool: Tool, at: Point, selection: Vec<i32>) -> Plan {
    let Some(element) = created_element(base, tool, at) else {
        return Plan::idle(selection);
    };
    let uid = element.get_uid();
    let mut changes = Changes::default();
    changes.set(base, element);
    Plan {
        handoff: Some(uid),
        ..changes.into_plan(base, vec![uid], "element creation")
    }
}

/// What a tap does. An armed aux, stock or module tool creates its element at
/// the tap under a default name and hands off to its name editor; the flow and
/// link tools draw only by dragging. A tap on an element selects it (a toggle
/// adds or removes it) and opens its details when it lands on the body or
/// label; a tap on a cloud, or on a flow's or link's end, selects the flow or
/// link. A tap on the empty canvas clears the selection, and a non-finite tap
/// plans nothing.
pub fn plan_tap(base: &BaseView, press: &Press) -> Plan {
    let current = &press.selection;
    if !press.point.is_finite() {
        return Plan::idle(current.clone());
    }
    let Some(hit) = press.hit else {
        return match press.tool {
            Some(tool @ (Tool::Aux | Tool::Stock | Tool::Module)) => {
                create_element_plan(base, tool, press.point, current.clone())
            }
            Some(Tool::Flow | Tool::Link) => Plan::idle(current.clone()),
            None => Plan::select(current, Vec::new()),
        };
    };
    let Some(element) = base.get(hit.uid) else {
        return Plan::idle(current.clone());
    };
    if press.toggle {
        let selection = if current.contains(&hit.uid) {
            current.iter().copied().filter(|u| *u != hit.uid).collect()
        } else {
            current.iter().copied().chain([hit.uid]).collect()
        };
        return Plan::select(current, selection);
    }
    let (selection, details) = match element {
        ViewElement::Cloud(cloud) if base.flow(cloud.flow_uid).is_some() => {
            (vec![cloud.flow_uid], false)
        }
        ViewElement::Flow(_) | ViewElement::Link(_)
            if matches!(hit.part, HitPart::Arrowhead | HitPart::Source) =>
        {
            (vec![hit.uid], false)
        }
        _ => (
            vec![hit.uid],
            matches!(hit.part, HitPart::Body | HitPart::Label),
        ),
    };
    Plan {
        details,
        ..Plan::select(current, selection)
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
enum Gesture {
    MoveSelection,
    /// A press on a sole selected flow's pipe or valve, latched by the first
    /// movement into `SlideValve` or `OffsetSegment`.
    Pipe {
        flow: i32,
        segment: usize,
    },
    SlideValve {
        flow: i32,
    },
    OffsetSegment {
        flow: i32,
        segment: usize,
    },
    FlowEndpoint {
        flow: i32,
        end: FlowEnd,
    },
    LinkEndpoint {
        link: i32,
    },
    LinkArc {
        link: i32,
    },
    /// Draw a flow out of a stock, or out of empty space (`None`).
    CreateFlow {
        from: Option<i32>,
    },
    CreateLink {
        from: i32,
    },
    CreateElement {
        tool: Tool,
    },
    Label {
        uid: i32,
    },
    RubberBand,
}

/// What a drag started on, for a host that draws affordances per gesture.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum GestureKind {
    MoveSelection,
    SlideValve,
    OffsetSegment,
    FlowEndpoint,
    LinkEndpoint,
    LinkArc,
    CreateFlow,
    CreateLink,
    CreateElement,
    Label,
    RubberBand,
}

impl GestureKind {
    pub const ALL: [GestureKind; 11] = [
        GestureKind::MoveSelection,
        GestureKind::SlideValve,
        GestureKind::OffsetSegment,
        GestureKind::FlowEndpoint,
        GestureKind::LinkEndpoint,
        GestureKind::LinkArc,
        GestureKind::CreateFlow,
        GestureKind::CreateLink,
        GestureKind::CreateElement,
        GestureKind::Label,
        GestureKind::RubberBand,
    ];
}

pub struct GestureSession {
    base: BaseView,
    gesture: Gesture,
    press: Point,
    pointer: PointerKind,
    /// The selection in effect for the drag.
    selection: Vec<i32>,
    target_slop: f64,
}

/// Start a drag. `None` when the press starts nothing, so the host pans: a
/// finger on the empty canvas with no tool armed, the link tool on the empty
/// canvas, or a press on an element the view lacks. Classification, in order:
///
/// - on the empty canvas: an aux, stock or module tool places a new element at
///   the pointer, the flow tool draws a flow out of empty space, and with no
///   tool a pencil or a pointer rubber-bands a selection. A finger starts
///   nothing there: a one-finger drag on the empty canvas pans, as it does on
///   every touch canvas;
/// - a label drags the label to another side;
/// - the link tool on a named element or an alias draws a link, and the flow
///   tool on a stock draws a flow;
/// - a flow's end drags that end, a link's arrowhead the link's end, and a
///   cloud that is not part of a larger selection drags its flow's end;
/// - otherwise the selection moves: the pressed element's own selection when it
///   is selected, else just the element (added to the selection with a
///   toggle). A sole link curves instead, and a sole flow's pipe or valve waits
///   for the first movement to latch slide or offset.
pub fn begin_drag(base: BaseView, press: Press) -> Option<GestureSession> {
    let (gesture, selection) = classify_drag(&base, &press)?;
    Some(GestureSession {
        base,
        gesture,
        press: press.point,
        pointer: press.pointer,
        selection,
        target_slop: if press.target_slop.is_finite() {
            press.target_slop.max(0.0)
        } else {
            0.0
        },
    })
}

fn classify_drag(base: &BaseView, press: &Press) -> Option<(Gesture, Vec<i32>)> {
    if !press.point.is_finite() {
        return None;
    }
    let current = press.selection.clone();
    let Some(hit) = press.hit else {
        return match press.tool {
            Some(tool @ (Tool::Aux | Tool::Stock | Tool::Module)) => {
                Some((Gesture::CreateElement { tool }, Vec::new()))
            }
            Some(Tool::Flow) => Some((Gesture::CreateFlow { from: None }, current)),
            Some(Tool::Link) => None,
            None if press.pointer == PointerKind::Touch => None,
            None => Some((Gesture::RubberBand, current)),
        };
    };
    let uid = hit.uid;
    let element = base.get(uid)?;
    if hit.part == HitPart::Label && label_side_of(element).is_some() {
        return Some((Gesture::Label { uid }, vec![uid]));
    }
    if hit.part == HitPart::Body && press.tool == Some(Tool::Link) && is_link_source(element) {
        return Some((Gesture::CreateLink { from: uid }, current));
    }
    if hit.part == HitPart::Body
        && press.tool == Some(Tool::Flow)
        && matches!(element, ViewElement::Stock(_))
    {
        return Some((Gesture::CreateFlow { from: Some(uid) }, current));
    }
    match (element, hit.part) {
        (ViewElement::Flow(_), HitPart::Source) => {
            return Some((
                Gesture::FlowEndpoint {
                    flow: uid,
                    end: FlowEnd::Source,
                },
                vec![uid],
            ));
        }
        (ViewElement::Flow(_), HitPart::Arrowhead) => {
            return Some((
                Gesture::FlowEndpoint {
                    flow: uid,
                    end: FlowEnd::Sink,
                },
                vec![uid],
            ));
        }
        (ViewElement::Link(_), HitPart::Arrowhead) => {
            return Some((Gesture::LinkEndpoint { link: uid }, vec![uid]));
        }
        _ => {}
    }
    let selected = current.contains(&uid);
    if let ViewElement::Cloud(cloud) = element
        && !(selected && current.len() > 1)
        && let Some(flow) = base.flow(cloud.flow_uid)
    {
        let n = flow.points.len();
        let end = if flow.points[0].attached_to_uid == Some(uid) {
            Some(FlowEnd::Source)
        } else if flow.points[n - 1].attached_to_uid == Some(uid) {
            Some(FlowEnd::Sink)
        } else {
            None
        };
        if let Some(end) = end {
            return Some((
                Gesture::FlowEndpoint {
                    flow: flow.uid,
                    end,
                },
                vec![flow.uid],
            ));
        }
    }
    let effective = match (selected, press.toggle) {
        (true, _) => current,
        (false, true) => current.into_iter().chain([uid]).collect(),
        (false, false) => vec![uid],
    };
    let gesture = match element {
        ViewElement::Link(_) if effective.len() == 1 => Gesture::LinkArc { link: uid },
        ViewElement::Flow(f) if effective.len() == 1 && f.points.len() >= 2 => Gesture::Pipe {
            flow: uid,
            segment: segment_nearest(&f.points, press.point),
        },
        _ => Gesture::MoveSelection,
    };
    Some((gesture, effective))
}

/// Move `selection` by `d`: the frame a move-selection drag plans for pointer
/// travel `d`, and what a host plans to nudge the selection from the keyboard,
/// so a nudge lands what dragging the selection that far would. Positioned
/// elements translate; a flow whose two terminals both move translates; a flow
/// with one moving terminal is routed to it (a stock carries the base face and
/// offset along, a cloud is re-centered on the routed endpoint); a selected flow
/// with no moving terminal slides its valve by the travel along its pipe; links
/// follow their moved endpoints, and have no position of their own to move. A
/// move in which some routed flow cannot hold G2-G6 (a cloud moved inside another
/// stock, say), or which overflows a coordinate, commits nothing, and a
/// non-finite `d` plans nothing.
pub fn plan_move(base: &BaseView, selection: &[i32], d: Point) -> Plan {
    if !d.is_finite() {
        return Plan::idle(selection.to_vec());
    }
    let mut changes = Changes::default();
    let mut moving: HashSet<i32> = HashSet::new();
    for &uid in selection {
        if let Some(moved) = base.get(uid).and_then(|e| translated(e, d)) {
            changes.set(base, moved);
            moving.insert(uid);
        }
    }
    let frame_stocks: Vec<Point> = base
        .stock_uids()
        .iter()
        .zip(base.stock_centers())
        .map(|(uid, c)| {
            if moving.contains(uid) {
                c.offset(d)
            } else {
                *c
            }
        })
        .collect();
    let mut flows: Vec<usize> = Vec::new();
    for &uid in &moving {
        if matches!(
            base.get(uid),
            Some(ViewElement::Stock(_) | ViewElement::Cloud(_))
        ) {
            flows.extend_from_slice(base.attached_flow_indices(uid));
        }
    }
    for &uid in selection {
        if base.flow(uid).is_some() {
            flows.extend(base.index_of(uid));
        }
    }
    flows.sort_unstable();
    flows.dedup();
    let mut next_uid = base.next_uid();
    let mut valid = true;
    let terminal_moves = |uid: Option<i32>| {
        uid.is_some_and(|u| {
            moving.contains(&u)
                && matches!(
                    base.get(u),
                    Some(ViewElement::Stock(_) | ViewElement::Cloud(_))
                )
        })
    };
    for index in flows {
        let ViewElement::Flow(flow) = &base.elements()[index] else {
            continue;
        };
        if flow.points.len() < 2 {
            continue;
        }
        let source_moves = terminal_moves(flow.points[0].attached_to_uid);
        let sink_moves = terminal_moves(flow.points[flow.points.len() - 1].attached_to_uid);
        if source_moves && sink_moves {
            changes.set(base, ViewElement::Flow(translate(flow, d)));
        } else if source_moves || sink_moves {
            let end = if source_moves {
                FlowEnd::Source
            } else {
                FlowEnd::Sink
            };
            valid &= route_moved_end(
                base,
                flow,
                end,
                d,
                &mut changes,
                &moving,
                &frame_stocks,
                &mut next_uid,
            );
        } else if selection.contains(&flow.uid) {
            let h = healed(base, flow, &mut next_uid);
            for cloud in h.clouds.iter().cloned() {
                changes.set(base, cloud);
            }
            changes.set(base, ViewElement::Flow(slide_valve(&h.flow, d)));
        }
    }
    let plan = changes.into_plan(base, selection.to_vec(), "move");
    if valid {
        plan
    } else {
        Plan {
            commit: CommitKind::None,
            label: "",
            ..plan
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn route_moved_end(
    base: &BaseView,
    flow: &Flow,
    end: FlowEnd,
    d: Point,
    changes: &mut Changes,
    moving: &HashSet<i32>,
    frame_stocks: &[Point],
    next_uid: &mut i32,
) -> bool {
    let h = healed(base, flow, next_uid);
    let n = h.flow.points.len();
    let (end_index, adjacent_index) = match end {
        FlowEnd::Source => (0, 1),
        FlowEnd::Sink => (n - 1, n - 2),
    };
    let Some(end_uid) = h.flow.points[end_index].attached_to_uid else {
        return false;
    };
    let terminals = h.terminals(base);
    let fixed = match end {
        FlowEnd::Source => terminals.sink,
        FlowEnd::Sink => terminals.source,
    };
    let (terminal, moved_cloud) = match h.get(base, end_uid) {
        Some(ViewElement::Stock(stock)) => {
            let from = Point::new(stock.x, stock.y);
            let t = stock_terminal(
                stock.uid,
                from.offset(d),
                Some(point_of(&h.flow.points[end_index])),
                Some(point_of(&h.flow.points[adjacent_index])),
                from,
            );
            (t, None)
        }
        Some(ViewElement::Cloud(cloud)) => {
            let at = Point::new(cloud.x + d.x, cloud.y + d.y);
            (
                free_terminal(at, Some(CloudRef { uid: cloud.uid, at })),
                Some(cloud.clone()),
            )
        }
        _ => return false,
    };
    let occupied = base.endpoints_on(&stock_uids_of(&[terminal, fixed]), flow.uid, |uid| {
        if moving.contains(&uid) {
            d
        } else {
            Point::new(0.0, 0.0)
        }
    });
    let g = route_end(&h.flow, end, terminal, fixed, &occupied, frame_stocks);
    for cloud in h.clouds {
        changes.set(base, cloud);
    }
    changes.move_clouds(base, &g.clouds);
    if let Some(cloud) = moved_cloud {
        let p = endpoint(&g.flow, end);
        changes.set(
            base,
            ViewElement::Cloud(Cloud {
                x: p.x,
                y: p.y,
                ..cloud
            }),
        );
    }
    let valid = flow_fault(
        &path_of(&g.flow),
        &with_end(end, terminal, fixed),
        frame_stocks,
    ) == Fault::None;
    changes.set(base, ViewElement::Flow(g.flow));
    valid
}

impl GestureSession {
    pub fn base(&self) -> &BaseView {
        &self.base
    }

    /// The gesture the drag started, `None` while a pipe press has not latched.
    pub fn kind(&self) -> Option<GestureKind> {
        Some(match self.gesture {
            Gesture::MoveSelection => GestureKind::MoveSelection,
            Gesture::Pipe { .. } => return None,
            Gesture::SlideValve { .. } => GestureKind::SlideValve,
            Gesture::OffsetSegment { .. } => GestureKind::OffsetSegment,
            Gesture::FlowEndpoint { .. } => GestureKind::FlowEndpoint,
            Gesture::LinkEndpoint { .. } => GestureKind::LinkEndpoint,
            Gesture::LinkArc { .. } => GestureKind::LinkArc,
            Gesture::CreateFlow { .. } => GestureKind::CreateFlow,
            Gesture::CreateLink { .. } => GestureKind::CreateLink,
            Gesture::CreateElement { .. } => GestureKind::CreateElement,
            Gesture::Label { .. } => GestureKind::Label,
            Gesture::RubberBand => GestureKind::RubberBand,
        })
    }

    /// Plan the frame with the pointer at `pointer` (model coordinates). A
    /// release commits the frame planned at the release point.
    pub fn frame(&mut self, pointer: Point) -> Plan {
        if !pointer.is_finite() {
            return Plan::idle(self.selection.clone());
        }
        if let Gesture::Pipe { flow, segment } = self.gesture {
            match self.latch(flow, segment, pointer) {
                Some(latched) => self.gesture = latched,
                None => return Plan::idle(self.selection.clone()),
            }
        }
        let d = pointer.minus(self.press);
        match self.gesture {
            Gesture::MoveSelection => plan_move(&self.base, &self.selection, d),
            Gesture::Pipe { .. } => Plan::idle(self.selection.clone()),
            Gesture::SlideValve { flow } => self.plan_slide_valve(flow, d),
            Gesture::OffsetSegment { flow, segment } => self.plan_offset_segment(flow, segment, d),
            Gesture::FlowEndpoint { flow, end } => self.plan_flow_endpoint(flow, end, pointer),
            Gesture::LinkEndpoint { link } => self.plan_link_endpoint(link, pointer),
            Gesture::LinkArc { link } => self.plan_link_arc(link, pointer),
            Gesture::CreateFlow { from } => self.plan_create_flow(from, pointer),
            Gesture::CreateLink { from } => self.plan_create_link(from, pointer),
            Gesture::CreateElement { tool } => {
                create_element_plan(&self.base, tool, pointer, Vec::new())
            }
            Gesture::Label { uid } => self.plan_label(uid, pointer),
            Gesture::RubberBand => self.plan_rubber_band(pointer),
        }
    }

    /// A perpendicular-dominant first movement offsets the pressed segment;
    /// anything else slides the valve. `None` until the pointer moves.
    fn latch(&self, flow: i32, segment: usize, pointer: Point) -> Option<Gesture> {
        let d = pointer.minus(self.press);
        if d.x == 0.0 && d.y == 0.0 {
            return None;
        }
        let Some(f) = self
            .base
            .flow(flow)
            .filter(|f| segment + 1 < f.points.len())
        else {
            return Some(Gesture::SlideValve { flow });
        };
        let (along, across) = match Axis::of_segment(
            point_of(&f.points[segment]),
            point_of(&f.points[segment + 1]),
        ) {
            Axis::X => (d.x.abs(), d.y.abs()),
            Axis::Y => (d.y.abs(), d.x.abs()),
        };
        Some(if across > along {
            Gesture::OffsetSegment { flow, segment }
        } else {
            Gesture::SlideValve { flow }
        })
    }

    fn idle(&self) -> Plan {
        Plan::idle(self.selection.clone())
    }

    /// Slide a sole selected flow's valve along its (healed) path by the pointer
    /// travel.
    fn plan_slide_valve(&self, flow: i32, d: Point) -> Plan {
        let base = &self.base;
        let Some(el) = base.flow(flow) else {
            return self.idle();
        };
        let mut next_uid = base.next_uid();
        let h = healed(base, el, &mut next_uid);
        let mut changes = Changes::default();
        for cloud in h.clouds.iter().cloned() {
            changes.set(base, cloud);
        }
        changes.set(base, ViewElement::Flow(slide_valve(&h.flow, d)));
        changes.into_plan(base, self.selection.clone(), "valve move")
    }

    /// Offset the pressed segment of a sole selected flow perpendicular to
    /// itself by the pointer travel. When healing an imported flow changed its
    /// points, the segment is the healed path's nearest to the press.
    fn plan_offset_segment(&self, flow: i32, segment: usize, d: Point) -> Plan {
        let base = &self.base;
        let Some(el) = base.flow(flow) else {
            return self.idle();
        };
        let mut next_uid = base.next_uid();
        let h = healed(base, el, &mut next_uid);
        let index = if h.flow == *el {
            segment
        } else {
            segment_nearest(&h.flow.points, self.press)
        };
        if index + 1 >= h.flow.points.len() {
            return self.idle();
        }
        let (axis, hold) = segment_hold(&path_of(&h.flow), index);
        let coordinate = hold
            + match axis {
                Axis::X => d.y,
                Axis::Y => d.x,
            };
        let g = offset_segment(
            &h.flow,
            index,
            coordinate,
            &h.terminals(base),
            base.stock_centers(),
        );
        let mut changes = Changes::default();
        for cloud in h.clouds {
            changes.set(base, cloud);
        }
        changes.move_clouds(base, &g.clouds);
        changes.set(base, ViewElement::Flow(g.flow));
        changes.into_plan(base, self.selection.clone(), "pipe move")
    }

    /// Drag one end of a flow. The end follows the pointer, keeping the grab
    /// offset; the pointer, not the end, finds targets:
    ///
    /// - over a stock the end routes onto it, and the drop is valid exactly when
    ///   the stock's variable exists, the stock is not the flow's other end, and
    ///   the routed flow holds G2-G6; a valid drop removes the end's old cloud;
    /// - over an invalid stock the end previews free at the pointer and commits
    ///   nothing (E6);
    /// - over empty space the end becomes, or stays, a cloud, committing when
    ///   the routed flow holds G2-G6.
    fn plan_flow_endpoint(&self, flow_uid: i32, end: FlowEnd, pointer: Point) -> Plan {
        let base = &self.base;
        let Some(el) = base.flow(flow_uid) else {
            return self.idle();
        };
        let mut next_uid = base.next_uid();
        let h = healed(base, el, &mut next_uid);
        let flow = &h.flow;
        let n = flow.points.len();
        let (end_index, adjacent_index) = match end {
            FlowEnd::Source => (0, 1),
            FlowEnd::Sink => (n - 1, n - 2),
        };
        let terminals = h.terminals(base);
        let fixed = match end {
            FlowEnd::Source => terminals.sink,
            FlowEnd::Sink => terminals.source,
        };
        let end_element = flow.points[end_index]
            .attached_to_uid
            .and_then(|uid| h.get(base, uid));
        // Every stock is in the way, the one this end leaves included: a route
        // keeping the old face's line would run straight through it.
        let obstacles = base.stock_centers();
        let mut changes = Changes::default();
        for cloud in h.clouds.iter().cloned() {
            changes.set(base, cloud);
        }
        let mut mark = None;
        if let Some(target) = base.stock_under(pointer, self.target_slop) {
            let center = Point::new(target.x, target.y);
            let terminal = if end_element.map(ViewElement::get_uid) == Some(target.uid) {
                stock_terminal(
                    target.uid,
                    center,
                    Some(point_of(&flow.points[end_index])),
                    Some(point_of(&flow.points[adjacent_index])),
                    center,
                )
            } else {
                target_stock_terminal(target.uid, center)
            };
            let occupied = base.endpoints_on(&stock_uids_of(&[terminal, fixed]), flow_uid, |_| {
                Point::new(0.0, 0.0)
            });
            let g = route_end(flow, end, terminal, fixed, &occupied, obstacles);
            let distinct = !matches!(fixed, Terminal::Stock { uid, .. } if uid == target.uid);
            let valid = distinct
                && base.variable_kind(&target.name) == Some(VariableKind::Stock)
                && flow_fault(
                    &path_of(&g.flow),
                    &with_end(end, terminal, fixed),
                    obstacles,
                ) == Fault::None;
            if valid {
                if let Some(ViewElement::Cloud(cloud)) = end_element {
                    changes.remove(cloud.uid);
                }
                changes.move_clouds(base, &g.clouds);
                changes.set(base, ViewElement::Flow(g.flow));
                return changes.into_drop(base, self.selection.clone(), "flow attach", target.uid);
            }
            mark = Some(Target {
                uid: target.uid,
                valid: false,
            });
        }
        let d = pointer.minus(self.press);
        let at = point_of(&flow.points[end_index]).offset(d);
        let cloud = match end_element {
            Some(ViewElement::Cloud(c)) => c.clone(),
            _ => {
                let uid = next_uid;
                Cloud {
                    uid,
                    flow_uid,
                    x: at.x,
                    y: at.y,
                    compat: None,
                }
            }
        };
        let terminal = free_terminal(
            at,
            Some(CloudRef {
                uid: cloud.uid,
                at: Point::new(cloud.x, cloud.y),
            }),
        );
        let occupied =
            base.endpoints_on(&stock_uids_of(&[fixed]), flow_uid, |_| Point::new(0.0, 0.0));
        let g = route_end(flow, end, terminal, fixed, &occupied, obstacles);
        let p = endpoint(&g.flow, end);
        let valid = mark.is_none()
            && flow_fault(
                &path_of(&g.flow),
                &with_end(end, terminal, fixed),
                obstacles,
            ) == Fault::None;
        changes.set(base, ViewElement::Flow(g.flow));
        changes.set(
            base,
            ViewElement::Cloud(Cloud {
                x: p.x,
                y: p.y,
                ..cloud
            }),
        );
        let plan = changes.into_plan(base, self.selection.clone(), "flow attach");
        Plan {
            target: mark,
            commit: if valid { plan.commit } else { CommitKind::None },
            label: if valid { plan.label } else { "" },
            ..plan
        }
    }

    /// Draw a new flow out of a stock (the route picks its face) or out of empty
    /// space (a cloud at the press), its sink at the pointer: onto a stock under
    /// the pointer when valid (a different stock from the source, whose variable
    /// exists, with a route holding G2-G6), else a cloud at the pointer. A valid
    /// drop selects the flow and hands off to its name editor; an invalid target
    /// commits nothing (E6).
    fn plan_create_flow(&self, from: Option<i32>, pointer: Point) -> Plan {
        let base = &self.base;
        let mut next_uid = base.next_uid();
        let flow_uid = next_uid;
        next_uid += 1;
        let draft = Flow {
            name: base.allocate_name("New Flow"),
            uid: flow_uid,
            x: self.press.x,
            y: self.press.y,
            label_side: LabelSide::Bottom,
            points: Vec::new(),
            compat: None,
            label_compat: None,
        };
        let (source, source_cloud) = match from {
            None => {
                let uid = next_uid;
                next_uid += 1;
                let cloud = Cloud {
                    uid,
                    flow_uid,
                    x: self.press.x,
                    y: self.press.y,
                    compat: None,
                };
                (
                    free_terminal(
                        self.press,
                        Some(CloudRef {
                            uid,
                            at: self.press,
                        }),
                    ),
                    Some(cloud),
                )
            }
            Some(stock) => match base.stock(stock) {
                Some(s) => (target_stock_terminal(s.uid, Point::new(s.x, s.y)), None),
                None => return self.idle(),
            },
        };
        let obstacles = base.stock_centers();
        let plan = |flow: Flow, sink_cloud: Option<Cloud>, valid: bool, target: Option<Target>| {
            let mut changes = Changes::default();
            if let Some(cloud) = &source_cloud {
                let p = point_of(&flow.points[0]);
                changes.set(
                    base,
                    ViewElement::Cloud(Cloud {
                        x: p.x,
                        y: p.y,
                        ..cloud.clone()
                    }),
                );
            }
            if let Some(cloud) = sink_cloud {
                let p = point_of(&flow.points[flow.points.len() - 1]);
                changes.set(
                    base,
                    ViewElement::Cloud(Cloud {
                        x: p.x,
                        y: p.y,
                        ..cloud
                    }),
                );
            }
            changes.set(base, ViewElement::Flow(flow));
            let (plan, refused) = changes.settle(base, vec![flow_uid], "flow creation");
            let edit = valid && plan.commit == CommitKind::Edit;
            Plan {
                // A stock the route allows the drop onto is refused where the
                // flow holds a number the scene cannot draw.
                target: target.map(|t| Target {
                    valid: t.valid && !refused,
                    ..t
                }),
                commit: if edit {
                    CommitKind::Edit
                } else {
                    CommitKind::None
                },
                handoff: edit.then_some(flow_uid),
                label: if edit { plan.label } else { "" },
                ..plan
            }
        };
        let mut mark = None;
        if let Some(target) = base.stock_under(pointer, self.target_slop) {
            let terminal = target_stock_terminal(target.uid, Point::new(target.x, target.y));
            let occupied = base.endpoints_on(&stock_uids_of(&[source, terminal]), flow_uid, |_| {
                Point::new(0.0, 0.0)
            });
            let g = route(
                source,
                terminal,
                &draft,
                FlowEnd::Source,
                &occupied,
                obstacles,
            );
            let distinct = !matches!(source, Terminal::Stock { uid, .. } if uid == target.uid);
            let valid = distinct
                && base.variable_kind(&target.name) == Some(VariableKind::Stock)
                && flow_fault(
                    &path_of(&g.flow),
                    &Terminals {
                        source,
                        sink: terminal,
                    },
                    obstacles,
                ) == Fault::None;
            if valid {
                return plan(
                    g.flow,
                    None,
                    true,
                    Some(Target {
                        uid: target.uid,
                        valid: true,
                    }),
                );
            }
            mark = Some(Target {
                uid: target.uid,
                valid: false,
            });
        }
        let sink_uid = next_uid;
        let sink_cloud = Cloud {
            uid: sink_uid,
            flow_uid,
            x: pointer.x,
            y: pointer.y,
            compat: None,
        };
        let sink = free_terminal(
            pointer,
            Some(CloudRef {
                uid: sink_uid,
                at: pointer,
            }),
        );
        let occupied = base.endpoints_on(&stock_uids_of(&[source]), flow_uid, |_| {
            Point::new(0.0, 0.0)
        });
        let g = route(source, sink, &draft, FlowEnd::Source, &occupied, obstacles);
        let valid = mark.is_none()
            && flow_fault(&path_of(&g.flow), &Terminals { source, sink }, obstacles) == Fault::None;
        plan(g.flow, Some(sink_cloud), valid, mark)
    }

    /// The element a dragged link's end is over, and whether the link may end
    /// there: never on its own source (no target at all), and never duplicating
    /// another link between the same two elements (a refused target).
    fn link_target(
        &self,
        from: i32,
        link: Option<i32>,
        pointer: Point,
    ) -> Option<(&ViewElement, bool)> {
        let element = self.base.link_target_under(pointer, self.target_slop)?;
        let uid = element.get_uid();
        if uid == from {
            return None;
        }
        let duplicate = self
            .base
            .touching_links(from)
            .any(|l| Some(l.uid) != link && l.from_uid == from && l.to_uid == uid);
        Some((element, !duplicate))
    }

    /// A link created or reattached with a touch is straight: a finger has no
    /// stable midpoint to curve through. A pencil or pointer curves it through
    /// the pointer.
    fn shape_to(&self, from: &ViewElement, to: &ViewElement, pointer: Point) -> LinkShape {
        match self.pointer {
            PointerKind::Touch => LinkShape::Straight,
            PointerKind::Pencil | PointerKind::Mouse => shape_through(
                visual_center(&self.base, from),
                visual_center(&self.base, to),
                pointer,
            ),
        }
    }

    /// Draw a link from `from`. Over a valid target it commits a link and
    /// selects it; anywhere else the preview draws a link to the pointer and the
    /// drop commits nothing.
    fn plan_create_link(&self, from: i32, pointer: Point) -> Plan {
        let base = &self.base;
        let Some(source) = base.get(from).filter(|e| is_link_source(e)) else {
            return self.idle();
        };
        let link_uid = base.next_uid();
        match self.link_target(from, None, pointer) {
            Some((target, true)) => {
                let mut changes = Changes::default();
                changes.set(
                    base,
                    ViewElement::Link(Link {
                        uid: link_uid,
                        from_uid: from,
                        to_uid: target.get_uid(),
                        shape: self.shape_to(source, target, pointer),
                        polarity: None,
                    }),
                );
                changes.into_drop(base, vec![link_uid], "link creation", target.get_uid())
            }
            t => Plan {
                target: t.map(|(e, _)| Target {
                    uid: e.get_uid(),
                    valid: false,
                }),
                dangling_link: Some(DanglingLink {
                    uid: link_uid,
                    from,
                    to: pointer,
                }),
                ..self.idle()
            },
        }
    }

    /// Drag an existing link's arrowhead. Over a valid target the link ends
    /// there. Over its own source, a refused target or empty space the preview
    /// draws it to the pointer and the drop commits nothing: dropping a link
    /// never deletes it.
    fn plan_link_endpoint(&self, link_uid: i32, pointer: Point) -> Plan {
        let base = &self.base;
        let Some(link) = base.link(link_uid) else {
            return self.idle();
        };
        let Some(source) = base.get(link.from_uid) else {
            return self.idle();
        };
        match self.link_target(link.from_uid, Some(link_uid), pointer) {
            Some((target, true)) => {
                let mut changes = Changes::default();
                changes.set(
                    base,
                    ViewElement::Link(Link {
                        to_uid: target.get_uid(),
                        shape: self.shape_to(source, target, pointer),
                        ..link.clone()
                    }),
                );
                changes.into_drop(
                    base,
                    self.selection.clone(),
                    "link attach",
                    target.get_uid(),
                )
            }
            t => Plan {
                removed: vec![link_uid],
                target: t.map(|(e, _)| Target {
                    uid: e.get_uid(),
                    valid: false,
                }),
                dangling_link: Some(DanglingLink {
                    uid: link_uid,
                    from: link.from_uid,
                    to: pointer,
                }),
                ..self.idle()
            },
        }
    }

    /// Curve a sole selected link through the pointer by dragging its body. A
    /// curve within `STRAIGHT_LINE_MAX` of the direct line is the straight link
    /// it draws as, so dragging a straight link's body back onto its line leaves
    /// it straight rather than an arc of the same drawing. With the pointer on
    /// the target's center no circle exists, and the link keeps its shape.
    fn plan_link_arc(&self, link_uid: i32, pointer: Point) -> Plan {
        let base = &self.base;
        let Some(link) = base.link(link_uid) else {
            return self.idle();
        };
        let (Some(from), Some(to)) = (base.get(link.from_uid), base.get(link.to_uid)) else {
            return self.idle();
        };
        let (from_center, to_center) = (visual_center(base, from), visual_center(base, to));
        if takeoff_through(from_center, to_center, pointer).is_none_or(|t| !t.is_finite()) {
            return self.idle();
        }
        let mut changes = Changes::default();
        changes.set(
            base,
            ViewElement::Link(Link {
                shape: shape_through(from_center, to_center, pointer),
                ..link.clone()
            }),
        );
        changes.into_plan(base, self.selection.clone(), "link arc")
    }

    /// Move a label to the side the pointer points it at; a side that does not
    /// change commits nothing.
    fn plan_label(&self, uid: i32, pointer: Point) -> Plan {
        let base = &self.base;
        let Some(element) = base.get(uid) else {
            return self.idle();
        };
        let (Some(center), Some(side)) = (position_of(element), label_side_of(element)) else {
            return self.idle();
        };
        let next = label_side_for_pointer(center, pointer);
        if next == side {
            return self.idle();
        }
        let mut changes = Changes::default();
        changes.extend_with(base, with_label_side(element, next));
        changes.into_plan(base, self.selection.clone(), "label move")
    }

    /// The rubber band's selection: stocks, clouds, flows (by valve), modules
    /// and aliases whose center lies in the rectangle, and auxes whose center
    /// lies in it or whose circle holds one of its corners. Links and groups are
    /// never rubber-band selected.
    fn plan_rubber_band(&self, pointer: Point) -> Plan {
        let (left, right) = (self.press.x.min(pointer.x), self.press.x.max(pointer.x));
        let (top, bottom) = (self.press.y.min(pointer.y), self.press.y.max(pointer.y));
        let inside = |x: f64, y: f64| x >= left && x <= right && y >= top && y <= bottom;
        let corners = [(left, top), (right, top), (left, bottom), (right, bottom)];
        let selection = self
            .base
            .elements()
            .iter()
            .filter(|e| match e {
                ViewElement::Cloud(c) => inside(c.x, c.y),
                ViewElement::Stock(s) => inside(s.x, s.y),
                ViewElement::Flow(f) => inside(f.x, f.y),
                ViewElement::Module(m) => inside(m.x, m.y),
                ViewElement::Alias(a) => inside(a.x, a.y),
                ViewElement::Aux(a) => {
                    inside(a.x, a.y)
                        || corners
                            .iter()
                            .any(|&(x, y)| (x - a.x).hypot(y - a.y) <= AUX_RADIUS)
                }
                ViewElement::Link(_) | ViewElement::Group(_) => false,
            })
            .map(ViewElement::get_uid)
            .collect();
        Plan {
            commit: CommitKind::Select,
            ..Plan::idle(selection)
        }
    }
}

impl Changes {
    fn extend_with(&mut self, base: &BaseView, element: Option<ViewElement>) {
        if let Some(element) = element {
            self.set(base, element);
        }
    }
}

#[cfg(test)]
#[path = "gesture_tests.rs"]
mod tests;
