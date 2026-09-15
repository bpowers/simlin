// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of gesture planning.
//!
//! Rows are derived from `GestureKind::ALL`: every gesture has a press that
//! starts it -- located through the production hit test, not a hand-built hit --
//! and a pointer to drag to. A row's committed plan is applied through the
//! production patch path (`ModelOperation::EditView`), and the committed view
//! must hold the strict flow invariants on every flow the edit routed, and
//! model/view agreement. Separate tables pin a drag back to its press (E1), the
//! refused drops (E6), and what a tap selects, creates and opens. Generated
//! scenes then drive stock moves and flow-end drags over many views.
//!
//! A nudge (`plan_move`) is the move-selection frame at an offset. Its rows are
//! derived from the kinds of view element, and a nudge of a selection some drag
//! moves must plan exactly what that drag's frame plans. Generated scenes nudge
//! random selections into edits that hold the invariants and change only what
//! the move reaches.

use rayon::prelude::*;

use crate::common::canonicalize;
use crate::datamodel;
use crate::diagram::connector::{ConnectorGeometry, connector_geometry};
use crate::diagram::elements::aux_geometry;
use crate::diagram::flow::flow_geometry;
use crate::diagram::label::label_bounds;
use crate::editing::hit::{HitIndex, hit_test};
use crate::editing::scene_gen::{Rng, strict_scene};
use crate::editing::test_support::{
    agreement_report, alias, apply_edit, aux, base_of, cloud, flow, labeled, link, load,
    project_of, stock, strict_report, view_of,
};
use crate::json;

use super::*;

const TOLERANCE: f64 = 10.0;

/// Two stocks joined by a straight flow, a cloud-to-cloud flow, an aux linked to
/// that flow, and a stock off to the side.
fn scene_elements() -> Vec<json::ViewElement> {
    vec![
        labeled(stock(1, 100.0, 100.0), "bottom"),
        labeled(stock(2, 400.0, 100.0), "bottom"),
        labeled(
            flow(3, (250.0, 100.0), &[(122.5, 100.0, 1), (377.5, 100.0, 2)]),
            "bottom",
        ),
        labeled(
            flow(4, (200.0, 300.0), &[(100.0, 300.0, 5), (300.0, 300.0, 6)]),
            "bottom",
        ),
        cloud(5, 4, 100.0, 300.0),
        cloud(6, 4, 300.0, 300.0),
        labeled(aux(7, 250.0, 450.0), "bottom"),
        link(8, 7, 4, None),
        labeled(stock(9, 600.0, 300.0), "bottom"),
    ]
}

fn scene() -> datamodel::Project {
    project_of(load(scene_elements()))
}

/// `scene` with an element of every kind it lacks, each clear of the rest: a
/// group, a module, and an alias of the aux.
fn every_kind_scene() -> datamodel::Project {
    let mut elements = scene_elements();
    elements.extend([
        json::ViewElement::Group(json::GroupViewElement {
            uid: 10,
            name: "g".to_string(),
            x: 500.0,
            y: 420.0,
            width: 160.0,
            height: 80.0,
            is_mdl_view_marker: false,
        }),
        labeled(
            json::ViewElement::Module(json::ModuleViewElement {
                uid: 11,
                name: "m".to_string(),
                x: 450.0,
                y: 620.0,
                label_side: String::new(),
            }),
            "bottom",
        ),
        labeled(alias(12, 7, 100.0, 550.0), "bottom"),
    ]);
    project_of(load(elements))
}

fn element(project: &datamodel::Project, uid: i32) -> &ViewElement {
    view_of(project)
        .iter()
        .find(|e| e.get_uid() == uid)
        .unwrap_or_else(|| panic!("no element {uid}"))
}

/// Where a flow's arrowhead is drawn.
fn arrowhead_tip(project: &datamodel::Project, flow_uid: i32) -> Point {
    let ViewElement::Flow(f) = element(project, flow_uid) else {
        panic!("{flow_uid} is not a flow");
    };
    let sink = f
        .points
        .last()
        .and_then(|p| p.attached_to_uid)
        .expect("an attached sink");
    let g = flow_geometry(f, element(project, sink), false).expect("a drawable flow");
    Point::new(g.arrowhead.tip.x, g.arrowhead.tip.y)
}

/// Where a link's arrowhead is drawn.
fn link_end(project: &datamodel::Project, link_uid: i32) -> Point {
    let ViewElement::Link(l) = element(project, link_uid) else {
        panic!("{link_uid} is not a link");
    };
    match connector_geometry(
        l,
        element(project, l.from_uid),
        element(project, l.to_uid),
        &|_| false,
    ) {
        ConnectorGeometry::Straight(g) => Point::new(g.end.x, g.end.y),
        ConnectorGeometry::Arc(g) => Point::new(g.end.x, g.end.y),
        ConnectorGeometry::Undrawable => panic!("link {link_uid} draws nothing"),
    }
}

/// The center of an aux's drawn label.
fn label_center(project: &datamodel::Project, aux_uid: i32) -> Point {
    let ViewElement::Aux(a) = element(project, aux_uid) else {
        panic!("{aux_uid} is not an aux");
    };
    let r = label_bounds(&aux_geometry(a, false).label);
    Point::new((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
}

struct Row {
    press: Point,
    tool: Option<Tool>,
    selection: Vec<i32>,
    hit: Option<(i32, HitPart)>,
    pointer: Point,
    commit: CommitKind,
}

/// The press that starts `kind` and a pointer to drag it to. Exhaustive, so a
/// new gesture does not compile until it has a row.
fn row(kind: GestureKind, project: &datamodel::Project) -> Row {
    let p = Point::new;
    let row = |press: Point,
               tool: Option<Tool>,
               selection: &[i32],
               hit: Option<(i32, HitPart)>,
               pointer: Point,
               commit: CommitKind| Row {
        press,
        tool,
        selection: selection.to_vec(),
        hit,
        pointer,
        commit,
    };
    match kind {
        GestureKind::MoveSelection => row(
            p(400.0, 100.0),
            None,
            &[],
            Some((2, HitPart::Body)),
            p(400.0, 180.0),
            CommitKind::Edit,
        ),
        GestureKind::SlideValve => row(
            p(200.0, 100.0),
            None,
            &[3],
            Some((3, HitPart::Body)),
            p(230.0, 101.0),
            CommitKind::Edit,
        ),
        GestureKind::OffsetSegment => row(
            p(200.0, 100.0),
            None,
            &[3],
            Some((3, HitPart::Body)),
            p(201.0, 140.0),
            CommitKind::Edit,
        ),
        GestureKind::FlowEndpoint => row(
            arrowhead_tip(project, 3),
            None,
            &[],
            Some((3, HitPart::Arrowhead)),
            p(330.0, 220.0),
            CommitKind::Edit,
        ),
        GestureKind::LinkEndpoint => row(
            link_end(project, 8),
            None,
            &[],
            Some((8, HitPart::Arrowhead)),
            p(250.0, 100.0),
            CommitKind::Edit,
        ),
        GestureKind::LinkArc => row(
            p(225.0, 375.0),
            None,
            &[8],
            Some((8, HitPart::Body)),
            p(260.0, 360.0),
            CommitKind::Edit,
        ),
        GestureKind::CreateFlow => row(
            p(400.0, 100.0),
            Some(Tool::Flow),
            &[],
            Some((2, HitPart::Body)),
            p(540.0, 100.0),
            CommitKind::Edit,
        ),
        GestureKind::CreateLink => row(
            p(250.0, 450.0),
            Some(Tool::Link),
            &[],
            Some((7, HitPart::Body)),
            p(250.0, 100.0),
            CommitKind::Edit,
        ),
        GestureKind::CreateElement => row(
            p(600.0, 600.0),
            Some(Tool::Aux),
            &[],
            None,
            p(620.0, 640.0),
            CommitKind::Edit,
        ),
        GestureKind::Label => row(
            label_center(project, 7),
            None,
            &[],
            Some((7, HitPart::Label)),
            p(180.0, 450.0),
            CommitKind::Edit,
        ),
        GestureKind::RubberBand => row(
            p(50.0, 250.0),
            None,
            &[],
            None,
            p(350.0, 350.0),
            CommitKind::Select,
        ),
    }
}

fn press(project: &datamodel::Project, r: &Row) -> Result<Press, String> {
    let hit = hit_test(project, "main", r.press, TOLERANCE)?;
    if hit.map(|h| (h.uid, h.part)) != r.hit {
        return Err(format!("the press hits {hit:?}, want {:?}", r.hit));
    }
    Ok(Press {
        point: r.press,
        hit,
        tool: r.tool,
        selection: r.selection.clone(),
        toggle: false,
        pointer: PointerKind::Mouse,
        target_slop: TOLERANCE,
    })
}

/// Apply a plan's edit and report what the committed view violates.
fn commit_report(project: &mut datamodel::Project, edit: &ViewEdit) -> String {
    if let Err(e) = apply_edit(project, edit) {
        return format!("the edit does not apply: {e}");
    }
    let routed: Vec<i32> = edit
        .upsert
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .map(ViewElement::get_uid)
        .collect();
    [
        strict_report(view_of(project), &routed),
        agreement_report(project),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn every_gesture_starts_from_its_press_and_commits_what_its_frame_previews() {
    let mut failures = Vec::new();
    for kind in GestureKind::ALL {
        let mut project = scene();
        let r = row(kind, &project);
        let press = match press(&project, &r) {
            Ok(press) => press,
            Err(e) => {
                failures.push(format!("{kind:?}: {e}"));
                continue;
            }
        };
        let Some(mut session) = begin_drag(base_of(&project), press) else {
            failures.push(format!("{kind:?}: the press starts no drag"));
            continue;
        };
        let plan = session.frame(r.pointer);
        if session.kind() != Some(kind) {
            failures.push(format!("{kind:?}: the drag latched {:?}", session.kind()));
        }
        if plan.commit != r.commit {
            failures.push(format!(
                "{kind:?}: commits {:?}, want {:?}",
                plan.commit, r.commit
            ));
        }
        // E2: the release plans the frame the preview showed.
        if session.frame(r.pointer) != plan {
            failures.push(format!(
                "{kind:?}: a second frame at the release differs from the preview"
            ));
        }
        match plan.edit(session.base()) {
            Some(edit) => {
                let report = commit_report(&mut project, &edit);
                if !report.is_empty() {
                    failures.push(format!("{kind:?}: {report}"));
                }
            }
            None if r.commit == CommitKind::Edit => {
                failures.push(format!("{kind:?}: an Edit commit with nothing to apply"));
            }
            None => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_drag_back_to_its_press_commits_nothing_except_placing_an_element() {
    let mut failures = Vec::new();
    for kind in GestureKind::ALL {
        let project = scene();
        let r = row(kind, &project);
        let Some(mut session) = press(&project, &r)
            .ok()
            .and_then(|p| begin_drag(base_of(&project), p))
        else {
            failures.push(format!("{kind:?}: the press starts no drag"));
            continue;
        };
        // Move first, so a pipe press latches the way the row's drag does.
        session.frame(r.pointer);
        let back = session.frame(r.press);
        // A creation tool places its element wherever the drag ends; every
        // other gesture back at its press changes nothing.
        let want_edit = kind == GestureKind::CreateElement;
        let edit = back.edit(session.base());
        if edit.is_some() != want_edit {
            failures.push(format!("{kind:?}: back at the press, edit {edit:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_refused_drop_marks_its_target_and_commits_nothing() {
    struct Drop {
        name: &'static str,
        press: Point,
        tool: Option<Tool>,
        hit: (i32, HitPart),
        pointer: Point,
        target: Option<(i32, bool)>,
        /// Remove this stock's variable first: a stock element whose variable
        /// the model lacks (imported data the editor accepts).
        without_variable: Option<&'static str>,
    }
    let base = scene();
    let p = Point::new;
    let drops = [
        Drop {
            name: "a flow's end onto the stock at its other end",
            press: arrowhead_tip(&base, 3),
            tool: None,
            hit: (3, HitPart::Arrowhead),
            pointer: p(100.0, 100.0),
            target: Some((1, false)),
            without_variable: None,
        },
        Drop {
            name: "a flow's end onto a stock with no variable",
            press: p(300.0, 300.0),
            tool: None,
            hit: (6, HitPart::Body),
            pointer: p(600.0, 300.0),
            target: Some((9, false)),
            without_variable: Some("s9"),
        },
        Drop {
            name: "a flow drawn from a stock back onto it",
            press: p(400.0, 100.0),
            tool: Some(Tool::Flow),
            hit: (2, HitPart::Body),
            pointer: p(410.0, 105.0),
            target: Some((2, false)),
            without_variable: None,
        },
        Drop {
            name: "a link onto its own source",
            press: p(250.0, 450.0),
            tool: Some(Tool::Link),
            hit: (7, HitPart::Body),
            pointer: p(252.0, 452.0),
            target: None,
            without_variable: None,
        },
        Drop {
            name: "a link duplicating an existing one",
            press: p(250.0, 450.0),
            tool: Some(Tool::Link),
            hit: (7, HitPart::Body),
            pointer: p(200.0, 300.0),
            target: Some((4, false)),
            without_variable: None,
        },
    ];
    let mut failures = Vec::new();
    for d in drops {
        let mut project = scene();
        if let Some(ident) = d.without_variable {
            project.models[0]
                .variables
                .retain(|v| canonicalize(v.get_ident()) != ident);
        }
        let r = Row {
            press: d.press,
            tool: d.tool,
            selection: Vec::new(),
            hit: Some(d.hit),
            pointer: d.pointer,
            commit: CommitKind::None,
        };
        let Some(mut session) = press(&project, &r)
            .ok()
            .and_then(|p| begin_drag(base_of(&project), p))
        else {
            failures.push(format!("{}: the press starts no drag", d.name));
            continue;
        };
        let plan = session.frame(d.pointer);
        let target = plan.target.map(|t| (t.uid, t.valid));
        if target != d.target
            || plan.commit != CommitKind::None
            || plan.edit(session.base()).is_some()
        {
            failures.push(format!(
                "{}: target {target:?} commit {:?}, want target {:?} and no commit",
                d.name, plan.commit, d.target
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_tap_selects_creates_and_opens_details() {
    struct Tap {
        name: &'static str,
        point: Point,
        tool: Option<Tool>,
        selection: &'static [i32],
        toggle: bool,
        commit: CommitKind,
        /// The selection after the tap; `None` for the element the tap creates.
        want: Option<&'static [i32]>,
        details: bool,
    }
    let base = scene();
    let p = Point::new;
    let taps = [
        Tap {
            name: "the empty canvas clears the selection",
            point: p(700.0, 700.0),
            tool: None,
            selection: &[2],
            toggle: false,
            commit: CommitKind::Select,
            want: Some(&[]),
            details: false,
        },
        Tap {
            name: "a creation tool places its element",
            point: p(700.0, 700.0),
            tool: Some(Tool::Aux),
            selection: &[2],
            toggle: false,
            commit: CommitKind::Edit,
            want: None,
            details: false,
        },
        Tap {
            name: "the flow tool draws only by dragging",
            point: p(700.0, 700.0),
            tool: Some(Tool::Flow),
            selection: &[2],
            toggle: false,
            commit: CommitKind::None,
            want: Some(&[2]),
            details: false,
        },
        Tap {
            name: "an element's body selects it and opens its details",
            point: p(400.0, 100.0),
            tool: None,
            selection: &[1],
            toggle: false,
            commit: CommitKind::Select,
            want: Some(&[2]),
            details: true,
        },
        Tap {
            name: "the sole selected element's body commits nothing and opens its details",
            point: p(400.0, 100.0),
            tool: None,
            selection: &[2],
            toggle: false,
            commit: CommitKind::None,
            want: Some(&[2]),
            details: true,
        },
        Tap {
            name: "a toggle adds an element",
            point: p(400.0, 100.0),
            tool: None,
            selection: &[1],
            toggle: true,
            commit: CommitKind::Select,
            want: Some(&[1, 2]),
            details: false,
        },
        Tap {
            name: "a toggle removes a selected element",
            point: p(400.0, 100.0),
            tool: None,
            selection: &[1, 2],
            toggle: true,
            commit: CommitKind::Select,
            want: Some(&[1]),
            details: false,
        },
        Tap {
            name: "a cloud selects its flow",
            point: p(100.0, 300.0),
            tool: None,
            selection: &[],
            toggle: false,
            commit: CommitKind::Select,
            want: Some(&[4]),
            details: false,
        },
        Tap {
            name: "a flow's end selects the flow",
            point: arrowhead_tip(&base, 3),
            tool: None,
            selection: &[],
            toggle: false,
            commit: CommitKind::Select,
            want: Some(&[3]),
            details: false,
        },
        Tap {
            name: "a label selects its element and opens its details",
            point: label_center(&base, 7),
            tool: None,
            selection: &[],
            toggle: false,
            commit: CommitKind::Select,
            want: Some(&[7]),
            details: true,
        },
    ];
    let mut failures = Vec::new();
    for t in taps {
        let mut project = scene();
        let hit = hit_test(&project, "main", t.point, TOLERANCE).expect("main has a view");
        let base = base_of(&project);
        let plan = plan_tap(
            &base,
            &Press {
                point: t.point,
                hit,
                tool: t.tool,
                selection: t.selection.to_vec(),
                toggle: t.toggle,
                pointer: PointerKind::Touch,
                target_slop: TOLERANCE,
            },
        );
        if plan.commit != t.commit || plan.details != t.details {
            failures.push(format!(
                "{}: commit {:?} details {}",
                t.name, plan.commit, plan.details
            ));
        }
        match t.want {
            Some(want) if plan.selection != want => {
                failures.push(format!(
                    "{}: selection {:?}, want {want:?}",
                    t.name, plan.selection
                ));
            }
            Some(_) => {}
            None => {
                let created = plan.handoff;
                if created.is_none() || plan.selection != created.into_iter().collect::<Vec<_>>() {
                    failures.push(format!(
                        "{}: handoff {created:?} selection {:?}",
                        t.name, plan.selection
                    ));
                }
                match plan.edit(&base) {
                    Some(edit) => {
                        let report = commit_report(&mut project, &edit);
                        if !report.is_empty() {
                            failures.push(format!("{}: {report}", t.name));
                        }
                    }
                    None => failures.push(format!("{}: creates nothing", t.name)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn presses_that_start_no_drag_leave_the_touch_to_the_host() {
    // Every arm of `begin_drag` that starts nothing. The positive twin of the
    // finger row, a pointer on the empty canvas rubber-banding, is the
    // `RubberBand` row of `every_gesture_starts_from_its_press_and_commits_what_its_frame_previews`.
    let project = scene();
    let canvas = Point::new(50.0, 250.0);
    let press = |point: Point, hit: Option<Hit>, tool: Option<Tool>, pointer: PointerKind| Press {
        point,
        hit,
        tool,
        selection: Vec::new(),
        toggle: false,
        pointer,
        target_slop: TOLERANCE,
    };
    let rows = [
        (
            "a finger on the empty canvas pans",
            press(canvas, None, None, PointerKind::Touch),
        ),
        (
            "the link tool on the empty canvas has nothing to draw from",
            press(canvas, None, Some(Tool::Link), PointerKind::Mouse),
        ),
        (
            "a press on an element the view lacks",
            press(
                canvas,
                Some(Hit {
                    uid: 99,
                    part: HitPart::Body,
                }),
                None,
                PointerKind::Mouse,
            ),
        ),
        (
            "a non-finite press",
            press(Point::new(f64::NAN, 250.0), None, None, PointerKind::Mouse),
        ),
    ];
    assert_eq!(
        hit_test(&project, "main", canvas, TOLERANCE).expect("main has a view"),
        None,
        "the canvas point is empty"
    );
    let failures: Vec<String> = rows
        .into_iter()
        .filter_map(|(name, press)| {
            begin_drag(base_of(&project), press).map(|s| format!("{name}: starts {:?}", s.kind()))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn random_delta(rng: &mut Rng) -> Point {
    let angle = rng.float(0.0, 2.0 * std::f64::consts::PI);
    let magnitude = rng.float(5.0, 250.0);
    Point::new(magnitude * angle.cos(), magnitude * angle.sin())
}

/// Drag from `at` to `to` on `project` and, when the frame commits an edit,
/// apply it to a copy and report what the committed view violates. Returns
/// whether an edit was applied.
fn drag_and_commit(
    project: &datamodel::Project,
    at: Point,
    to: Point,
    context: &str,
    failures: &mut Vec<String>,
) -> bool {
    let Ok(Some(hit)) = hit_test(project, "main", at, TOLERANCE) else {
        return false;
    };
    let press = Press {
        point: at,
        hit: Some(hit),
        tool: None,
        selection: Vec::new(),
        toggle: false,
        pointer: PointerKind::Touch,
        target_slop: TOLERANCE,
    };
    let Some(mut session) = begin_drag(base_of(project), press) else {
        return false;
    };
    let plan = session.frame(to);
    let Some(edit) = plan.edit(session.base()) else {
        return false;
    };
    let mut committed = project.clone();
    let report = commit_report(&mut committed, &edit);
    if !report.is_empty() {
        failures.push(format!("{context}: {report}"));
    }
    true
}

#[test]
fn generated_scenes_move_stocks_and_drag_flow_ends_into_edits_that_hold_the_invariants() {
    const SEEDS: u32 = 60;
    let results: Vec<(usize, Vec<String>)> = (1..=SEEDS)
        .into_par_iter()
        .map(|seed| {
            let scene = strict_scene(seed);
            let project = project_of(scene.elements);
            let mut rng = Rng::new(seed.wrapping_mul(31).wrapping_add(7));
            let mut failures = Vec::new();
            let mut applied = 0;
            let view = view_of(&project).to_vec();
            let stocks: Vec<(i32, Point)> = view
                .iter()
                .filter_map(|e| match e {
                    ViewElement::Stock(s) => Some((s.uid, Point::new(s.x, s.y))),
                    _ => None,
                })
                .collect();
            for &(uid, center) in &stocks {
                let d = random_delta(&mut rng);
                let to = Point::new(center.x + d.x, center.y + d.y);
                applied += usize::from(drag_and_commit(
                    &project,
                    center,
                    to,
                    &format!("seed {seed} move S{uid}"),
                    &mut failures,
                ));
            }
            for element in &view {
                let ViewElement::Cloud(c) = element else {
                    continue;
                };
                let at = Point::new(c.x, c.y);
                let d = random_delta(&mut rng);
                let to = Point::new(at.x + d.x, at.y + d.y);
                applied += usize::from(drag_and_commit(
                    &project,
                    at,
                    to,
                    &format!("seed {seed} drag cloud {}", c.uid),
                    &mut failures,
                ));
                for &(uid, center) in &stocks {
                    applied += usize::from(drag_and_commit(
                        &project,
                        at,
                        center,
                        &format!("seed {seed} cloud {} onto S{uid}", c.uid),
                        &mut failures,
                    ));
                }
            }
            (applied, failures)
        })
        .collect();
    let applied: usize = results.iter().map(|(a, _)| a).sum();
    let failures: Vec<&String> = results.iter().flat_map(|(_, f)| f).collect();
    assert!(
        applied > 200,
        "only {applied} edits applied over {SEEDS} seeds"
    );
    assert!(
        failures.is_empty(),
        "{} of {applied} committed edits violate\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// A selection nudged by `d`, and what landing it does.
struct Nudge {
    name: &'static str,
    selection: Vec<i32>,
    d: Point,
    /// Whether the nudge lands an edit.
    lands: bool,
    /// Elements whose position moves by exactly `d`.
    translated: Vec<i32>,
    /// The selected element a move-selection drag of the selection is pressed
    /// on, for a selection a drag moves.
    drag_from: Option<i32>,
}

/// The nudge of a lone element of `element`'s kind. Exhaustive, so a new kind
/// of element does not compile until it has a row.
fn lone_nudge(element: &ViewElement) -> Nudge {
    let uid = element.get_uid();
    let alone = |name: &'static str, lands: bool| Nudge {
        name,
        selection: vec![uid],
        d: Point::new(10.0, 0.0),
        lands,
        translated: Vec::new(),
        drag_from: None,
    };
    let dragged = |name: &'static str| Nudge {
        translated: vec![uid],
        drag_from: Some(uid),
        ..alone(name, true)
    };
    match element {
        ViewElement::Stock(_) => dragged("a stock"),
        ViewElement::Aux(_) => dragged("an aux"),
        ViewElement::Module(_) => dragged("a module"),
        ViewElement::Alias(_) => dragged("an alias"),
        ViewElement::Group(_) => dragged("a group"),
        // Its flow's end is routed onto it, which re-centers it on the routed
        // endpoint; a drag of a lone cloud drags its flow's end instead.
        ViewElement::Cloud(_) => alone("a cloud", true),
        // The valve slides along the pipe; a drag of a lone flow latches a slide
        // or an offset on its first movement (the own-clouds test below).
        ViewElement::Flow(_) => alone("a flow", true),
        // A link has no position of its own, and a drag of a lone link curves it.
        ViewElement::Link(_) => alone("a link", false),
    }
}

/// A point whose production hit is `uid`'s body: its position when the hit
/// there is, else the first such point of a grid around it.
fn body_point(project: &datamodel::Project, uid: i32) -> Point {
    let index = HitIndex::new(project, "main").expect("main has a view");
    let on_body = |p: Point| {
        index.hit(p, TOLERANCE)
            == Some(Hit {
                uid,
                part: HitPart::Body,
            })
    };
    let at = position_of(element(project, uid)).expect("a positioned element");
    if on_body(at) {
        return at;
    }
    (0..81)
        .flat_map(|i| {
            (0..81).map(move |j| {
                Point::new(
                    at.x - 100.0 + 2.5 * f64::from(i),
                    at.y - 100.0 + 2.5 * f64::from(j),
                )
            })
        })
        .find(|&p| on_body(p))
        .unwrap_or_else(|| panic!("no point lands on {uid}'s body"))
}

/// The uids a move of `selection` may change: the selection, every flow it holds
/// or with an end on what it holds, those flows' clouds, and the links touching
/// any of them. A committed move leaves everything else as it was (E4).
fn may_change(view: &[ViewElement], selection: &[i32]) -> HashSet<i32> {
    let mut reached: HashSet<i32> = selection.iter().copied().collect();
    for element in view {
        let ViewElement::Flow(f) = element else {
            continue;
        };
        let ends_on_selection = [f.points.first(), f.points.last()]
            .into_iter()
            .flatten()
            .any(|p| p.attached_to_uid.is_some_and(|u| selection.contains(&u)));
        if selection.contains(&f.uid) || ends_on_selection {
            reached.insert(f.uid);
            reached.extend(view.iter().filter_map(|e| match e {
                ViewElement::Cloud(c) if c.flow_uid == f.uid => Some(c.uid),
                _ => None,
            }));
        }
    }
    let links: Vec<i32> = view
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l)
                if reached.contains(&l.from_uid) || reached.contains(&l.to_uid) =>
            {
                Some(l.uid)
            }
            _ => None,
        })
        .collect();
    reached.extend(links);
    reached
}

/// The base elements a move of `selection` may not change that `committed`
/// changed or dropped.
fn changed_outside_the_move(
    before: &[ViewElement],
    committed: &[ViewElement],
    selection: &[i32],
) -> Vec<i32> {
    let reached = may_change(before, selection);
    before
        .iter()
        .filter(|b| !reached.contains(&b.get_uid()))
        .filter(|b| committed.iter().find(|e| e.get_uid() == b.get_uid()) != Some(*b))
        .map(ViewElement::get_uid)
        .collect()
}

/// What a nudge row's plan violates, empty when it holds.
fn nudge_report(project: &datamodel::Project, n: &Nudge) -> Vec<String> {
    let base = base_of(project);
    let plan = plan_move(&base, &n.selection, n.d);
    let mut failures = Vec::new();
    if plan.selection != n.selection {
        failures.push(format!("{}: selection {:?}", n.name, plan.selection));
    }
    let edit = plan.edit(&base);
    if edit.is_some() != n.lands {
        failures.push(format!("{}: lands {edit:?}, want {}", n.name, n.lands));
    }
    if let Some(edit) = &edit {
        if plan.label != "move" {
            failures.push(format!("{}: label {:?}", n.name, plan.label));
        }
        let mut committed = project.clone();
        let report = commit_report(&mut committed, edit);
        if !report.is_empty() {
            failures.push(format!("{}: {report}", n.name));
        }
        let outside = changed_outside_the_move(view_of(project), view_of(&committed), &n.selection);
        if !outside.is_empty() {
            failures.push(format!(
                "{}: changes {outside:?}, which the move does not reach",
                n.name
            ));
        }
        for &uid in &n.translated {
            let before = position_of(element(project, uid));
            let after = position_of(element(&committed, uid));
            if after != before.map(|p| p.offset(n.d)) {
                failures.push(format!(
                    "{}: {uid} moves from {before:?} to {after:?}, not by the offset",
                    n.name
                ));
            }
        }
    }
    if let Some(from) = n.drag_from {
        let at = body_point(project, from);
        let press = Press {
            point: at,
            hit: Some(Hit {
                uid: from,
                part: HitPart::Body,
            }),
            tool: None,
            selection: n.selection.clone(),
            toggle: false,
            pointer: PointerKind::Mouse,
            target_slop: TOLERANCE,
        };
        match begin_drag(base_of(project), press) {
            Some(mut session) => {
                let pointer = at.offset(n.d);
                let frame = session.frame(pointer);
                if session.kind() != Some(GestureKind::MoveSelection) {
                    failures.push(format!("{}: the drag latched {:?}", n.name, session.kind()));
                }
                // The travel the frame reads, as the frame computes it.
                if frame != plan_move(&base, &n.selection, pointer.minus(at)) {
                    failures.push(format!(
                        "{}: the drag's frame plans other than the nudge",
                        n.name
                    ));
                }
            }
            None => failures.push(format!("{}: the press starts no drag", n.name)),
        }
    }
    failures
}

#[test]
fn a_lone_element_of_each_kind_nudges_as_its_move_plans() {
    let project = every_kind_scene();
    let mut kinds = HashSet::new();
    let mut failures = Vec::new();
    for element in view_of(&project) {
        if kinds.insert(std::mem::discriminant(element)) {
            failures.extend(nudge_report(&project, &lone_nudge(element)));
        }
    }
    assert_eq!(kinds.len(), 8, "the scene holds an element of every kind");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_nudged_selection_lands_what_its_drag_plans_or_nothing() {
    let project = every_kind_scene();
    let p = Point::new;
    let nudge = |name: &'static str,
                 selection: &[i32],
                 d: Point,
                 lands: bool,
                 translated: &[i32],
                 drag_from: Option<i32>| Nudge {
        name,
        selection: selection.to_vec(),
        d,
        lands,
        translated: translated.to_vec(),
        drag_from,
    };
    let rows = [
        nudge(
            "a flow with both its clouds translates whole",
            &[4, 5, 6],
            p(10.0, 20.0),
            true,
            &[4, 5, 6],
            Some(5),
        ),
        nudge(
            "two stocks carry the flow between them",
            &[1, 2],
            p(0.0, 30.0),
            true,
            &[1, 2, 3],
            Some(1),
        ),
        nudge(
            "a stock re-routes its flow, and a moved aux's link follows",
            &[1, 7],
            p(0.0, 30.0),
            true,
            &[1, 7],
            Some(1),
        ),
        nudge(
            "a move putting a cloud inside a stock commits nothing",
            &[6, 7],
            p(300.0, 0.0),
            false,
            &[],
            Some(7),
        ),
        nudge(
            "an empty selection moves nothing",
            &[],
            p(10.0, 0.0),
            false,
            &[],
            None,
        ),
        nudge(
            "a zero offset moves nothing",
            &[7],
            p(0.0, 0.0),
            false,
            &[],
            Some(7),
        ),
        nudge(
            "a non-finite offset moves nothing",
            &[7],
            p(f64::NAN, 0.0),
            false,
            &[],
            None,
        ),
        nudge(
            "a uid the view lacks moves nothing",
            &[99],
            p(10.0, 0.0),
            false,
            &[],
            None,
        ),
    ];
    let failures: Vec<String> = rows
        .iter()
        .flat_map(|n| nudge_report(&project, n))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_lone_flow_with_its_own_clouds_nudges_along_its_pipe_as_its_valve_drag_does() {
    let project = scene();
    let base = base_of(&project);
    // On flow 4's pipe, between its source cloud and its valve.
    let at = Point::new(150.0, 300.0);
    let hit = hit_test(&project, "main", at, TOLERANCE).expect("main has a view");
    assert_eq!(
        hit.map(|h| (h.uid, h.part)),
        Some((4, HitPart::Body)),
        "the press lands on the pipe"
    );
    let drag = |d: Point| {
        let press = Press {
            point: at,
            hit,
            tool: None,
            selection: vec![4],
            toggle: false,
            pointer: PointerKind::Mouse,
            target_slop: TOLERANCE,
        };
        let mut session = begin_drag(base_of(&project), press)
            .expect("a press on a sole selected flow's pipe starts a drag");
        let plan = session.frame(at.offset(d));
        (session.kind(), plan.edit(session.base()))
    };
    let upserted =
        |edit: &ViewEdit| -> Vec<i32> { edit.upsert.iter().map(ViewElement::get_uid).collect() };

    // Along the pipe the drag latches a valve slide, and the nudge lands the
    // same edit: the valve moves along, and both clouds stay.
    let along = Point::new(20.0, 0.0);
    let (kind, dragged) = drag(along);
    assert_eq!(kind, Some(GestureKind::SlideValve));
    let nudged = plan_move(&base, &[4], along)
        .edit(&base)
        .expect("the nudge lands");
    assert_eq!(
        Some(&nudged),
        dragged.as_ref(),
        "the nudge lands what the valve drag lands"
    );
    assert_eq!(
        upserted(&nudged),
        [4],
        "only the flow changes; its clouds stay"
    );
    let Some(ViewElement::Flow(f)) = nudged.upsert.first() else {
        panic!("the flow is upserted");
    };
    assert_eq!(
        (f.x, f.y),
        (220.0, 300.0),
        "the valve slides by the travel along the pipe"
    );

    // Across the pipe the drag latches a segment offset, which carries both
    // clouds with the pipe. A nudge has no pressed segment, and a valve slides
    // only along its pipe, so it lands nothing.
    let across = Point::new(0.0, 20.0);
    let (kind, dragged) = drag(across);
    assert_eq!(kind, Some(GestureKind::OffsetSegment));
    let dragged = dragged.expect("the offset lands");
    assert!(
        [5, 6].iter().all(|c| upserted(&dragged).contains(c)),
        "the offset carries both clouds: {:?}",
        upserted(&dragged)
    );
    assert_eq!(
        plan_move(&base, &[4], across).edit(&base),
        None,
        "a nudge across a straight pipe lands nothing"
    );
}

#[test]
fn generated_scenes_nudge_selections_into_edits_that_hold_the_invariants() {
    const SEEDS: u32 = 40;
    const NUDGES: usize = 12;
    let keys = [
        Point::new(1.0, 0.0),
        Point::new(-1.0, 0.0),
        Point::new(0.0, 1.0),
        Point::new(0.0, -1.0),
        Point::new(10.0, 0.0),
        Point::new(-10.0, 0.0),
        Point::new(0.0, 10.0),
        Point::new(0.0, -10.0),
    ];
    let results: Vec<(usize, Vec<String>)> = (1..=SEEDS)
        .into_par_iter()
        .map(|seed| {
            let project = project_of(strict_scene(seed).elements);
            let base = base_of(&project);
            let view = view_of(&project);
            let movable: Vec<i32> = view
                .iter()
                .filter(|e| !matches!(e, ViewElement::Link(_)))
                .map(ViewElement::get_uid)
                .collect();
            let mut rng = Rng::new(seed.wrapping_mul(131).wrapping_add(17));
            let (mut applied, mut failures) = (0, Vec::new());
            for n in 0..NUDGES {
                let size = rng.int(1.0, 4.0) as usize;
                let selection: Vec<i32> = (0..size).map(|_| rng.pick(&movable)).collect();
                // Arrow keys, and now and then a travel as large as a drag's.
                let d = if n % 3 == 2 {
                    random_delta(&mut rng)
                } else {
                    rng.pick(&keys)
                };
                let Some(edit) = plan_move(&base, &selection, d).edit(&base) else {
                    continue;
                };
                applied += 1;
                let context = format!("seed {seed} nudge {selection:?} by ({}, {})", d.x, d.y);
                let mut committed = project.clone();
                let report = commit_report(&mut committed, &edit);
                if !report.is_empty() {
                    failures.push(format!("{context}: {report}"));
                }
                let outside = changed_outside_the_move(view, view_of(&committed), &selection);
                if !outside.is_empty() {
                    failures.push(format!(
                        "{context}: changes {outside:?}, which the move does not reach"
                    ));
                }
            }
            (applied, failures)
        })
        .collect();
    let applied: usize = results.iter().map(|(a, _)| a).sum();
    let failures: Vec<&String> = results.iter().flat_map(|(_, f)| f).collect();
    assert!(
        applied > 300,
        "only {applied} nudges landed over {SEEDS} seeds"
    );
    assert!(
        failures.is_empty(),
        "{} of {applied} landed nudges violate\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
