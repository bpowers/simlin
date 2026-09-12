// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The flow geometry invariants G1-G8 of
//! `docs/design-plans/2026-09-10-diagram-editing-core.md` as a checker over a
//! stock-and-flow view: the test oracle for everything that produces flow
//! geometry through the editing core.
//!
//! Every arm has its own `FlowArm` variant and its own small function, so a
//! change to one definition in the plan stays local, and tests derive their
//! fixture rows from `FlowArm::ALL`. The checker shares no geometry with the
//! core (`editing::{terminal, validity, path}`), so a test that routes with the
//! core and checks with this cannot agree with itself by construction; its
//! constants are its own literals, pinned against the core's by a test. It
//! follows the editor's checker (`src/diagram/tests/support/flow-invariants.ts`)
//! arm for arm.
//!
//! Modes. `Strict` is what a committed edit must produce for every flow it
//! routed. `Tolerant` is what an input view must satisfy for the editor to
//! accept it at all: imported and legacy views render unmodified and a flow is
//! healed only when an edit routes it, and heal repairs every geometric arm, so
//! tolerant mode keeps only the structural G1 arms whose violation leaves
//! nothing to heal from.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{Cloud, Flow, FlowPoint, Stock};
use crate::diagram::constants::{FLOW_ARROWHEAD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};

pub(crate) const CORNER_CLEARANCE: f64 = 3.0;
pub(crate) const MIN_SEGMENT: f64 = 10.0;
pub(crate) const VALVE_CLAMP_MARGIN: f64 = 10.0;
pub(crate) const MIN_SINK_SEGMENT: f64 = FLOW_ARROWHEAD_RADIUS + 7.5;
pub(crate) const GEOMETRY_EPSILON: f64 = 1e-6;

const HALF_WIDTH: f64 = STOCK_WIDTH / 2.0;
const HALF_HEIGHT: f64 = STOCK_HEIGHT / 2.0;

/// One arm of the flow invariants.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FlowArm {
    MinPoints,
    NonFinite,
    UnattachedEndpoint,
    DanglingAttachment,
    AttachmentKind,
    ForeignCloud,
    InteriorAttached,
    NonPositiveUid,
    SourceIsSink,
    Diagonal,
    ZeroLength,
    Collinear,
    ShortStub,
    ShortRiser,
    ShortSink,
    OffFace,
    CornerClearance,
    NotPerpendicular,
    Inward,
    SegmentThroughTerminal,
    CloudInsideStock,
    CloudOffEndpoint,
    ValveOffPath,
    ValveMargin,
}

impl FlowArm {
    pub const ALL: [FlowArm; 24] = [
        FlowArm::MinPoints,
        FlowArm::NonFinite,
        FlowArm::UnattachedEndpoint,
        FlowArm::DanglingAttachment,
        FlowArm::AttachmentKind,
        FlowArm::ForeignCloud,
        FlowArm::InteriorAttached,
        FlowArm::NonPositiveUid,
        FlowArm::SourceIsSink,
        FlowArm::Diagonal,
        FlowArm::ZeroLength,
        FlowArm::Collinear,
        FlowArm::ShortStub,
        FlowArm::ShortRiser,
        FlowArm::ShortSink,
        FlowArm::OffFace,
        FlowArm::CornerClearance,
        FlowArm::NotPerpendicular,
        FlowArm::Inward,
        FlowArm::SegmentThroughTerminal,
        FlowArm::CloudInsideStock,
        FlowArm::CloudOffEndpoint,
        FlowArm::ValveOffPath,
        FlowArm::ValveMargin,
    ];

    /// The plan's name for the arm, `invariant.arm`.
    pub fn name(self) -> &'static str {
        match self {
            FlowArm::MinPoints => "G1.minPoints",
            FlowArm::NonFinite => "G1.nonFinite",
            FlowArm::UnattachedEndpoint => "G1.unattachedEndpoint",
            FlowArm::DanglingAttachment => "G1.danglingAttachment",
            FlowArm::AttachmentKind => "G1.attachmentKind",
            FlowArm::ForeignCloud => "G1.foreignCloud",
            FlowArm::InteriorAttached => "G1.interiorAttached",
            FlowArm::NonPositiveUid => "G1.nonPositiveUid",
            FlowArm::SourceIsSink => "G1.sourceIsSink",
            FlowArm::Diagonal => "G2.diagonal",
            FlowArm::ZeroLength => "G3.zeroLength",
            FlowArm::Collinear => "G3.collinear",
            FlowArm::ShortStub => "G3.shortStub",
            FlowArm::ShortRiser => "G3.shortRiser",
            FlowArm::ShortSink => "G3.shortSink",
            FlowArm::OffFace => "G4.offFace",
            FlowArm::CornerClearance => "G4.cornerClearance",
            FlowArm::NotPerpendicular => "G5.notPerpendicular",
            FlowArm::Inward => "G5.inward",
            FlowArm::SegmentThroughTerminal => "G6.segmentThroughTerminal",
            FlowArm::CloudInsideStock => "G6.cloudInsideStock",
            FlowArm::CloudOffEndpoint => "G7.cloudOffEndpoint",
            FlowArm::ValveOffPath => "G8.valveOffPath",
            FlowArm::ValveMargin => "G8.valveMargin",
        }
    }

    /// Whether tolerant mode reports the arm. Two structural arms are
    /// deliberately absent: an unattached endpoint, because the Vensim importer
    /// emits flows with no attachment and the editor must accept them; and a flow
    /// whose source and sink are the same element, which tolerating is the
    /// permissive direction -- the editor must never fail on one, and routing it
    /// is heal's job, not the input gate's.
    pub fn tolerant(self) -> bool {
        matches!(
            self,
            FlowArm::MinPoints
                | FlowArm::NonFinite
                | FlowArm::DanglingAttachment
                | FlowArm::AttachmentKind
                | FlowArm::ForeignCloud
                | FlowArm::InteriorAttached
        )
    }
}

#[derive(Clone, Copy)]
pub enum Mode<'a> {
    /// What a committed edit must produce. With `routed`, the full arm set
    /// applies only to those flows (the rest of the view may carry imported
    /// violations) and every other flow gets the tolerant arms; `None` means
    /// every flow was routed. Non-positive uids are reported whatever `routed`
    /// holds: they are a property of a committed view, not of a flow.
    Strict {
        routed: Option<&'a HashSet<i32>>,
    },
    Tolerant,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct FlowViolation {
    pub arm: FlowArm,
    /// The flow's uid; for `NonPositiveUid`, the offending element's.
    pub uid: i32,
    /// The measured quantities the arm fired on, by name.
    pub numbers: Vec<(&'static str, f64)>,
    pub message: String,
}

impl FlowViolation {
    pub fn number(&self, key: &str) -> Option<f64> {
        self.numbers
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
    }
}

/// One line per violation: the arm, the uid, the numbers, and the message.
pub fn format_violations(violations: &[FlowViolation]) -> String {
    violations
        .iter()
        .map(|v| {
            let numbers: Vec<String> = v.numbers.iter().map(|(k, n)| format!("{k}: {n}")).collect();
            format!(
                "{} uid={} {{{}}} {}",
                v.arm.name(),
                v.uid,
                numbers.join(", "),
                v.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check every flow of a view's elements.
pub fn check_flow_invariants(elements: &[ViewElement], mode: Mode<'_>) -> Vec<FlowViolation> {
    let by_uid: HashMap<i32, &ViewElement> = elements.iter().map(|e| (e.get_uid(), e)).collect();
    let mut out = Vec::new();
    if matches!(mode, Mode::Strict { .. }) {
        check_non_positive_uids(elements, &mut out);
    }
    for element in elements {
        let ViewElement::Flow(flow) = element else {
            continue;
        };
        let strict = match mode {
            Mode::Strict { routed } => routed.is_none_or(|r| r.contains(&flow.uid)),
            Mode::Tolerant => false,
        };
        let mut report = Reporter {
            out: &mut out,
            uid: flow.uid,
            strict,
        };
        let Some(ctx) = check_structure(flow, &by_uid, &mut report) else {
            continue;
        };
        check_orthogonal(&ctx, &mut report);
        check_normalized(&ctx, &mut report);
        check_face_attachment(&ctx, &mut report);
        check_perpendicular_exit(&ctx, &mut report);
        check_no_body_crossing(&ctx, elements, &mut report);
        check_cloud_coincidence(&ctx, &mut report);
        check_valve_on_path(&ctx, &mut report);
    }
    out
}

struct Reporter<'o> {
    out: &'o mut Vec<FlowViolation>,
    uid: i32,
    strict: bool,
}

impl Reporter<'_> {
    fn report(&mut self, arm: FlowArm, numbers: &[(&'static str, f64)], message: String) {
        if self.strict || arm.tolerant() {
            self.out.push(FlowViolation {
                arm,
                uid: self.uid,
                numbers: numbers.to_vec(),
                message,
            });
        }
    }
}

#[derive(Clone, Copy)]
enum TerminalElement<'a> {
    Stock(&'a Stock),
    Cloud(&'a Cloud),
}

struct FlowContext<'a> {
    flow: &'a Flow,
    source: Option<TerminalElement<'a>>,
    sink: Option<TerminalElement<'a>>,
}

impl<'a> FlowContext<'a> {
    fn ends(&self) -> [(usize, Option<TerminalElement<'a>>); 2] {
        [(0, self.source), (self.flow.points.len() - 1, self.sink)]
    }
}

// ---------------------------------------------------------------------------
// G1 structure

/// "No uid <= 0 in a committed view": a planner stages sentinel uids for
/// in-creation elements, so this is a property of committed views only and
/// applies to every element, routed or not.
fn check_non_positive_uids(elements: &[ViewElement], out: &mut Vec<FlowViolation>) {
    for element in elements {
        let uid = element.get_uid();
        if uid <= 0 {
            out.push(FlowViolation {
                arm: FlowArm::NonPositiveUid,
                uid,
                numbers: vec![("uid", f64::from(uid))],
                message: format!("{} element has uid {uid}", kind_of(element)),
            });
        }
    }
}

/// The resolved terminals, or `None` when the flow is too broken for any
/// geometric arm to mean anything (fewer than two points, or a non-finite
/// coordinate every later computation would only echo).
fn check_structure<'a>(
    flow: &'a Flow,
    by_uid: &HashMap<i32, &'a ViewElement>,
    report: &mut Reporter<'_>,
) -> Option<FlowContext<'a>> {
    let pts = &flow.points;
    let non_finite = [flow.x, flow.y]
        .into_iter()
        .chain(pts.iter().flat_map(|p| [p.x, p.y]))
        .filter(|v| !v.is_finite())
        .count();
    if non_finite > 0 {
        report.report(
            FlowArm::NonFinite,
            &[("count", non_finite as f64)],
            format!("{non_finite} non-finite coordinate(s)"),
        );
    }
    let n = pts.len();
    if n < 2 {
        report.report(
            FlowArm::MinPoints,
            &[("points", n as f64)],
            format!("flow has {n} point(s)"),
        );
        return None;
    }
    for (i, p) in pts.iter().enumerate().take(n - 1).skip(1) {
        if let Some(attached) = p.attached_to_uid {
            report.report(
                FlowArm::InteriorAttached,
                &[("index", i as f64), ("attachedToUid", f64::from(attached))],
                format!("interior point {i} is attached"),
            );
        }
    }
    let source = resolve_terminal(flow, 0, by_uid, report);
    let sink = resolve_terminal(flow, n - 1, by_uid, report);
    // A cloud at both ends is also M3's (a cloud is an endpoint of its flow
    // exactly once); this arm owns the stock self-loop the planner refuses.
    if source.is_some() && sink.is_some() && pts[0].attached_to_uid == pts[n - 1].attached_to_uid {
        let uid = pts[0].attached_to_uid.unwrap_or(0);
        report.report(
            FlowArm::SourceIsSink,
            &[("attachedToUid", f64::from(uid))],
            format!("source and sink are both element {uid}"),
        );
    }
    if non_finite > 0 {
        return None;
    }
    Some(FlowContext { flow, source, sink })
}

fn resolve_terminal<'a>(
    flow: &Flow,
    index: usize,
    by_uid: &HashMap<i32, &'a ViewElement>,
    report: &mut Reporter<'_>,
) -> Option<TerminalElement<'a>> {
    let end = if index == 0 { "source" } else { "sink" };
    let Some(attached) = flow.points[index].attached_to_uid else {
        report.report(
            FlowArm::UnattachedEndpoint,
            &[("endIndex", index as f64)],
            format!("{end} endpoint is unattached"),
        );
        return None;
    };
    match by_uid.get(&attached).copied() {
        None => {
            report.report(
                FlowArm::DanglingAttachment,
                &[
                    ("endIndex", index as f64),
                    ("attachedToUid", f64::from(attached)),
                ],
                format!("{end} references missing uid {attached}"),
            );
            None
        }
        Some(ViewElement::Stock(stock)) => Some(TerminalElement::Stock(stock)),
        Some(ViewElement::Cloud(cloud)) if cloud.flow_uid != flow.uid => {
            report.report(
                FlowArm::ForeignCloud,
                &[
                    ("endIndex", index as f64),
                    ("cloudUid", f64::from(cloud.uid)),
                    ("cloudFlowUid", f64::from(cloud.flow_uid)),
                ],
                format!(
                    "{end} cloud {} belongs to flow {}",
                    cloud.uid, cloud.flow_uid
                ),
            );
            None
        }
        Some(ViewElement::Cloud(cloud)) => Some(TerminalElement::Cloud(cloud)),
        Some(other) => {
            report.report(
                FlowArm::AttachmentKind,
                &[
                    ("endIndex", index as f64),
                    ("attachedToUid", f64::from(attached)),
                ],
                format!("{end} is attached to a {}", kind_of(other)),
            );
            None
        }
    }
}

fn kind_of(element: &ViewElement) -> &'static str {
    match element {
        ViewElement::Aux(_) => "aux",
        ViewElement::Stock(_) => "stock",
        ViewElement::Flow(_) => "flow",
        ViewElement::Link(_) => "link",
        ViewElement::Module(_) => "module",
        ViewElement::Alias(_) => "alias",
        ViewElement::Cloud(_) => "cloud",
        ViewElement::Group(_) => "group",
    }
}

// ---------------------------------------------------------------------------
// G2 orthogonal

fn check_orthogonal(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    for (i, w) in ctx.flow.points.windows(2).enumerate() {
        if orientation(&w[0], &w[1]) == Orientation::Diagonal {
            report.report(
                FlowArm::Diagonal,
                &[
                    ("segment", i as f64),
                    ("dx", w[1].x - w[0].x),
                    ("dy", w[1].y - w[0].y),
                ],
                format!("segment {i} is not axis-aligned"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// G3 normalized

fn check_normalized(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    let pts = &ctx.flow.points;
    let segments = pts.len() - 1;
    for i in 0..segments {
        if orientation(&pts[i], &pts[i + 1]) == Orientation::Zero {
            report.report(
                FlowArm::ZeroLength,
                &[("segment", i as f64)],
                format!("segment {i} has zero length"),
            );
        }
    }
    for i in 0..segments.saturating_sub(1) {
        let a = orientation(&pts[i], &pts[i + 1]);
        let b = orientation(&pts[i + 1], &pts[i + 2]);
        if matches!(a, Orientation::Horizontal | Orientation::Vertical) && a == b {
            report.report(
                FlowArm::Collinear,
                &[("segment", (i + 1) as f64)],
                format!("segments {i} and {} are collinear", i + 1),
            );
        }
    }
    if !terminals_leave_room(ctx) {
        return;
    }
    let e = GEOMETRY_EPSILON;
    for i in 0..segments {
        let length = distance(&pts[i], &pts[i + 1]);
        if length <= e {
            continue;
        }
        let (arm, minimum, what) = if i == segments - 1 {
            (FlowArm::ShortSink, MIN_SINK_SEGMENT, "final segment")
        } else if i == 0 {
            (FlowArm::ShortStub, MIN_SEGMENT, "first segment")
        } else {
            (FlowArm::ShortRiser, MIN_SEGMENT, "interior segment")
        };
        if length < minimum - e {
            report.report(
                arm,
                &[
                    ("segment", i as f64),
                    ("length", length),
                    ("minimum", minimum),
                ],
                format!("{what} {i} is {length}px"),
            );
        }
    }
}

/// G3's "whenever the terminals leave room": the source body inflated by
/// `MIN_SEGMENT` and the sink body inflated by `MIN_SINK_SEGMENT` are disjoint.
/// A missing terminal (an unattached endpoint, tolerated input) cannot crowd.
fn terminals_leave_room(ctx: &FlowContext<'_>) -> bool {
    let (Some(source), Some(sink)) = (ctx.source, ctx.sink) else {
        return true;
    };
    !body_of(source)
        .inflate(MIN_SEGMENT)
        .overlaps(body_of(sink).inflate(MIN_SINK_SEGMENT))
}

// ---------------------------------------------------------------------------
// G4 face attachment

fn check_face_attachment(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    for (index, terminal) in ctx.ends() {
        let Some(TerminalElement::Stock(stock)) = terminal else {
            continue;
        };
        let p = &ctx.flow.points[index];
        let sides = sides_of(p, stock);
        if sides.is_empty() {
            report.report(
                FlowArm::OffFace,
                &[
                    ("endIndex", index as f64),
                    ("dx", p.x - stock.x),
                    ("dy", p.y - stock.y),
                ],
                format!("endpoint is not on a face of stock {}", stock.uid),
            );
            continue;
        }
        let clearance = sides
            .iter()
            .map(|&side| side_clearance(p, stock, side))
            .fold(f64::INFINITY, f64::min);
        if clearance < CORNER_CLEARANCE - GEOMETRY_EPSILON {
            report.report(
                FlowArm::CornerClearance,
                &[
                    ("endIndex", index as f64),
                    ("clearance", clearance),
                    ("minimum", CORNER_CLEARANCE),
                ],
                format!(
                    "endpoint is {clearance}px from a corner of stock {}",
                    stock.uid
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// G5 perpendicular exit

fn check_perpendicular_exit(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    let pts = &ctx.flow.points;
    for (index, terminal) in ctx.ends() {
        let Some(TerminalElement::Stock(stock)) = terminal else {
            continue;
        };
        let p = &pts[index];
        let sides = sides_of(p, stock);
        // "That face" is undefined for an off-face endpoint; G4.offFace owns it.
        if sides.is_empty() {
            continue;
        }
        // The adjacent segment is the first one of positive length, so a
        // coincident point (G3.zeroLength) does not hide the direction the pipe
        // actually takes.
        let Some(neighbor) = first_distinct_neighbor(pts, index) else {
            continue;
        };
        let (dx, dy) = (neighbor.x - p.x, neighbor.y - p.y);
        if sides.iter().any(|&side| leaves_outward(side, dx, dy)) {
            continue;
        }
        let perpendicular = sides.iter().any(|&side| match side {
            Side::Left | Side::Right => dy.abs() <= GEOMETRY_EPSILON,
            Side::Top | Side::Bottom => dx.abs() <= GEOMETRY_EPSILON,
        });
        let names: Vec<&str> = sides.iter().map(|&side| side.name()).collect();
        let (arm, what) = if perpendicular {
            (FlowArm::Inward, "points into the stock")
        } else {
            (FlowArm::NotPerpendicular, "is not perpendicular to it")
        };
        report.report(
            arm,
            &[("endIndex", index as f64), ("dx", dx), ("dy", dy)],
            format!("segment at the {} face {what}", names.join("/")),
        );
    }
}

fn leaves_outward(side: Side, dx: f64, dy: f64) -> bool {
    let horizontal = dy.abs() <= GEOMETRY_EPSILON;
    let vertical = dx.abs() <= GEOMETRY_EPSILON;
    match side {
        Side::Left => horizontal && dx < 0.0,
        Side::Right => horizontal && dx > 0.0,
        Side::Top => vertical && dy < 0.0,
        Side::Bottom => vertical && dy > 0.0,
    }
}

fn first_distinct_neighbor(pts: &[FlowPoint], index: usize) -> Option<&FlowPoint> {
    let p = &pts[index];
    if index == 0 {
        pts[1..]
            .iter()
            .find(|q| orientation(p, q) != Orientation::Zero)
    } else {
        pts[..index]
            .iter()
            .rev()
            .find(|q| orientation(p, q) != Orientation::Zero)
    }
}

// ---------------------------------------------------------------------------
// G6 no body crossing

/// "No cloud center lies inside a stock" is read as ANY stock of the view. Read
/// as this flow's other terminal the clause could never fire: a cloud inside
/// that stock makes the inflated terminal bodies overlap, which is exactly the
/// precondition that exempts G6.
fn check_no_body_crossing(
    ctx: &FlowContext<'_>,
    elements: &[ViewElement],
    report: &mut Reporter<'_>,
) {
    // With one terminal missing (tolerated input) there is no pair to overlap,
    // so the precondition holds.
    if let (Some(source), Some(sink)) = (ctx.source, ctx.sink)
        && body_of(source)
            .inflate(MIN_SEGMENT)
            .overlaps(body_of(sink).inflate(MIN_SEGMENT))
    {
        return;
    }
    let pts = &ctx.flow.points;
    let mut stocks: Vec<&Stock> = Vec::new();
    for (_, terminal) in ctx.ends() {
        if let Some(TerminalElement::Stock(stock)) = terminal
            && !stocks.iter().any(|s| s.uid == stock.uid)
        {
            stocks.push(stock);
        }
    }
    for stock in &stocks {
        for (i, w) in pts.windows(2).enumerate() {
            if segment_enters_interior(&w[0], &w[1], stock) {
                report.report(
                    FlowArm::SegmentThroughTerminal,
                    &[("segment", i as f64), ("stockUid", f64::from(stock.uid))],
                    format!("segment {i} crosses stock {}", stock.uid),
                );
            }
        }
    }
    for (_, terminal) in ctx.ends() {
        let Some(TerminalElement::Cloud(cloud)) = terminal else {
            continue;
        };
        for element in elements {
            if let ViewElement::Stock(stock) = element
                && strictly_inside(cloud.x, cloud.y, stock)
            {
                report.report(
                    FlowArm::CloudInsideStock,
                    &[
                        ("cloudUid", f64::from(cloud.uid)),
                        ("stockUid", f64::from(stock.uid)),
                    ],
                    format!("cloud {} lies inside stock {}", cloud.uid, stock.uid),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// G7 cloud coincidence

fn check_cloud_coincidence(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    for (index, terminal) in ctx.ends() {
        let Some(TerminalElement::Cloud(cloud)) = terminal else {
            continue;
        };
        let p = &ctx.flow.points[index];
        let d = (p.x - cloud.x).hypot(p.y - cloud.y);
        if d > GEOMETRY_EPSILON {
            report.report(
                FlowArm::CloudOffEndpoint,
                &[("endIndex", index as f64), ("distance", d)],
                format!("cloud {} is {d}px from its endpoint", cloud.uid),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// G8 valve on path

/// "When the path is long enough" is read as: long enough for a position at
/// least `VALVE_CLAMP_MARGIN` from both ends to exist, i.e. at least two
/// margins. The margin is arc length from the path's ends, not per segment.
fn check_valve_on_path(ctx: &FlowContext<'_>, report: &mut Reporter<'_>) {
    let pts = &ctx.flow.points;
    let (vx, vy) = (ctx.flow.x, ctx.flow.y);
    let mut best = f64::INFINITY;
    let mut arc_position = 0.0;
    let mut traversed = 0.0;
    for w in pts.windows(2) {
        let length = distance(&w[0], &w[1]);
        let (d, t) = distance_to_segment(vx, vy, &w[0], &w[1]);
        if d < best {
            best = d;
            arc_position = traversed + t * length;
        }
        traversed += length;
    }
    if best > GEOMETRY_EPSILON {
        report.report(
            FlowArm::ValveOffPath,
            &[("distance", best)],
            format!("valve is {best}px off the path"),
        );
        return;
    }
    let path_length = traversed;
    if path_length < 2.0 * VALVE_CLAMP_MARGIN {
        return;
    }
    let from_end = arc_position.min(path_length - arc_position);
    if from_end < VALVE_CLAMP_MARGIN - GEOMETRY_EPSILON {
        report.report(
            FlowArm::ValveMargin,
            &[
                ("arcPosition", arc_position),
                ("pathLength", path_length),
                ("margin", VALVE_CLAMP_MARGIN),
            ],
            format!("valve is {from_end}px from an end of the path"),
        );
    }
}

// ---------------------------------------------------------------------------
// Geometry, independent of the core

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
}

#[derive(Clone, Copy)]
struct Rect {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

impl Rect {
    fn inflate(self, by: f64) -> Rect {
        Rect {
            min_x: self.min_x - by,
            max_x: self.max_x + by,
            min_y: self.min_y - by,
            max_y: self.max_y + by,
        }
    }

    /// Touching rectangles do not overlap: the preconditions exempt crowding,
    /// and two bodies exactly `MIN_SEGMENT` apart leave exactly enough room.
    fn overlaps(self, other: Rect) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }
}

fn body_of(terminal: TerminalElement<'_>) -> Rect {
    match terminal {
        TerminalElement::Stock(s) => Rect {
            min_x: s.x - HALF_WIDTH,
            max_x: s.x + HALF_WIDTH,
            min_y: s.y - HALF_HEIGHT,
            max_y: s.y + HALF_HEIGHT,
        },
        TerminalElement::Cloud(c) => Rect {
            min_x: c.x,
            max_x: c.x,
            min_y: c.y,
            max_y: c.y,
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Orientation {
    Zero,
    Horizontal,
    Vertical,
    Diagonal,
}

fn orientation(a: &FlowPoint, b: &FlowPoint) -> Orientation {
    let flat_x = (b.x - a.x).abs() <= GEOMETRY_EPSILON;
    let flat_y = (b.y - a.y).abs() <= GEOMETRY_EPSILON;
    match (flat_x, flat_y) {
        (true, true) => Orientation::Zero,
        (false, true) => Orientation::Horizontal,
        (true, false) => Orientation::Vertical,
        (false, false) => Orientation::Diagonal,
    }
}

fn distance(a: &FlowPoint, b: &FlowPoint) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// The distance from `(px, py)` to the segment, and the parameter of the
/// nearest point.
fn distance_to_segment(px: f64, py: f64, a: &FlowPoint, b: &FlowPoint) -> (f64, f64) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((px - a.x) * dx + (py - a.y) * dy) / length_squared).clamp(0.0, 1.0)
    };
    ((px - (a.x + t * dx)).hypot(py - (a.y + t * dy)), t)
}

fn sides_of(p: &FlowPoint, stock: &Stock) -> Vec<Side> {
    let dx = p.x - stock.x;
    let dy = p.y - stock.y;
    let e = GEOMETRY_EPSILON;
    let mut sides = Vec::new();
    if (dx.abs() - HALF_WIDTH).abs() <= e && dy.abs() <= HALF_HEIGHT + e {
        sides.push(if dx > 0.0 { Side::Right } else { Side::Left });
    }
    if (dy.abs() - HALF_HEIGHT).abs() <= e && dx.abs() <= HALF_WIDTH + e {
        sides.push(if dy > 0.0 { Side::Bottom } else { Side::Top });
    }
    sides
}

fn side_clearance(p: &FlowPoint, stock: &Stock, side: Side) -> f64 {
    match side {
        Side::Left | Side::Right => HALF_HEIGHT - (p.y - stock.y).abs(),
        Side::Top | Side::Bottom => HALF_WIDTH - (p.x - stock.x).abs(),
    }
}

fn strictly_inside(x: f64, y: f64, stock: &Stock) -> bool {
    (x - stock.x).abs() < HALF_WIDTH - GEOMETRY_EPSILON
        && (y - stock.y).abs() < HALF_HEIGHT - GEOMETRY_EPSILON
}

/// Liang-Barsky clip against the stock rectangle inset by `GEOMETRY_EPSILON`,
/// so a segment that starts on a face or runs along an edge line is not
/// "through".
fn segment_enters_interior(a: &FlowPoint, b: &FlowPoint, stock: &Stock) -> bool {
    let e = GEOMETRY_EPSILON;
    let min_x = stock.x - HALF_WIDTH + e;
    let max_x = stock.x + HALF_WIDTH - e;
    let min_y = stock.y - HALF_HEIGHT + e;
    let max_y = stock.y + HALF_HEIGHT - e;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t0 = 0.0;
    let mut t1 = 1.0;
    let mut clip = |p: f64, q: f64| -> bool {
        if p == 0.0 {
            return q > 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return false;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return false;
            }
            if r < t1 {
                t1 = r;
            }
        }
        true
    };
    if !clip(-dx, a.x - min_x)
        || !clip(dx, max_x - a.x)
        || !clip(-dy, a.y - min_y)
        || !clip(dy, max_y - a.y)
    {
        return false;
    }
    (t1 - t0) * dx.hypot(dy) > e
}

#[cfg(test)]
#[path = "invariants_tests.rs"]
mod tests;
