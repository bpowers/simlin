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

use rayon::prelude::*;

use crate::common::canonicalize;
use crate::datamodel;
use crate::diagram::connector::{ConnectorGeometry, connector_geometry};
use crate::diagram::elements::aux_geometry;
use crate::diagram::flow::flow_geometry;
use crate::diagram::label::label_bounds;
use crate::editing::hit::hit_test;
use crate::editing::scene_gen::{Rng, strict_scene};
use crate::editing::test_support::{
    agreement_report, apply_edit, aux, base_of, cloud, flow, labeled, link, load, project_of,
    stock, strict_report, view_of,
};

use super::*;

const TOLERANCE: f64 = 10.0;

/// Two stocks joined by a straight flow, a cloud-to-cloud flow, an aux linked to
/// that flow, and a stock off to the side.
fn scene() -> datamodel::Project {
    project_of(load(vec![
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
    ]))
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
