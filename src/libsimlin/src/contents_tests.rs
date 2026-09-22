// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The hit index a project caches is never older than its datamodel, and its
//! revision advances with every change.
//!
//! Every mutable borrow of the datamodel drops the cached indexes and advances
//! the revision (`ProjectContents`'s `DerefMut`), so no entry point can mutate
//! without invalidating or without counting the change. The rows pin both for
//! the entry points that mutate a project's datamodel, found among its lock
//! sites (`grep -n 'datamodel.lock()' src`): `simlin_project_apply_patch`,
//! through its view-only apply and its staged commit,
//! `simlin_project_replace_contents`, `simlin_project_add_model` and
//! `simlin_project_diagram_sync`; every other site reads. Each row warms the
//! index, mutates through the entry point, and requires that the index was
//! dropped, that the revision advanced, and that the hit test answers at every
//! probe point what an index built from the new datamodel answers. For every
//! row but `add_model`, which changes no model's view, the old index answered
//! differently somewhere, so a stale index fails the row.
//!
//! The reads keep the index and the revision, pinned for a row per kind of
//! read: a dry-run and a rejected patch, a simulation, diagnostics, a scene,
//! serialization, the editing planners, and a tool session's call, change
//! report and run. A read that
//! dropped the index would only cost a rebuild, but a read that advanced the
//! revision would tell every host caching against it that the project had
//! changed when it had not.
//!
//! A copy shares the datamodel until either side is edited. The rows take a
//! project and a copy of it (`simlin_project_replace_contents` into
//! `simlin_project_new`), mutate one side through each mutating entry point,
//! and require that the other side is exactly as it was; the reads, a dry run
//! and a rejected patch, leave the two sharing one datamodel.
//!
//! A hit test locks the datamodel as the planners do, so a press issued while an
//! edit holds the project waits for the edit to land. The contract test holds an
//! edit at its staging point, under both project locks, and presses from another
//! thread where the edit creates an aux: the hit test answers nothing while the
//! edit holds the project, then answers the aux, and the tap planned from that
//! hit selects it.

use std::ffi::CString;
use std::ptr;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use simlin_engine::editing::{self, Point};

use crate::ffi_error::SimlinError;
use crate::*;

const STOCK: i32 = 1;
const TOLERANCE: f64 = 10.0;

/// A cloud filling a stock through a flow, and an aux linked to the flow, with
/// the stock at `(x, y)`.
fn contents_json(stock_x: f64, stock_y: f64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "name": "editing",
        "simSpecs": {"startTime": 0.0, "endTime": 10.0, "dt": "1"},
        "models": [{
            "name": "main",
            "stocks": [{"name": "population", "initialEquation": "10", "inflows": ["births"], "outflows": []}],
            "flows": [{"name": "births", "equation": "population * rate"}],
            "auxiliaries": [{"name": "rate", "equation": "0.1"}],
            "views": [{"elements": [
                {"type": "stock", "uid": STOCK, "name": "population", "x": stock_x, "y": stock_y},
                {"type": "flow", "uid": 2, "name": "births", "x": 38.75, "y": 100.0, "points": [
                    {"x": 0.0, "y": 100.0, "attachedToUid": 3},
                    {"x": 77.5, "y": 100.0, "attachedToUid": STOCK}
                ]},
                {"type": "cloud", "uid": 3, "flowUid": 2, "x": 0.0, "y": 100.0},
                {"type": "aux", "uid": 4, "name": "rate", "x": 40.0, "y": 200.0},
                {"type": "link", "uid": 5, "fromUid": 4, "toUid": 2}
            ]}]
        }]
    }))
    .unwrap()
}

unsafe fn expect_no_error(err: *mut SimlinError, what: &str) {
    if !err.is_null() {
        let code = simlin_error_get_code(err);
        simlin_error_free(err);
        panic!("{what} failed with {code:?}");
    }
}

unsafe fn open(stock_x: f64, stock_y: f64) -> *mut SimlinProject {
    let bytes = contents_json(stock_x, stock_y);
    let mut err = ptr::null_mut();
    let proj = simlin_project_open_json(bytes.as_ptr(), bytes.len(), 0, &mut err);
    expect_no_error(err, "opening the project");
    proj
}

unsafe fn main_model(proj: *mut SimlinProject) -> *mut SimlinModel {
    let name = CString::new("main").unwrap();
    let mut err = ptr::null_mut();
    let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
    expect_no_error(err, "getting main");
    model
}

unsafe fn has_index(proj: *mut SimlinProject) -> bool {
    (*proj).datamodel.lock().unwrap().has_hit_index("main")
}

/// The project's revision, through the FFI.
unsafe fn revision(proj: *mut SimlinProject) -> u64 {
    let (mut revision, mut err) = (0, ptr::null_mut());
    simlin_project_get_revision(proj, &mut revision, &mut err);
    expect_no_error(err, "reading the revision");
    revision
}

/// The points a row asks about: a grid over the diagram and around it.
fn probes() -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for i in 0..30 {
        for j in 0..30 {
            out.push((-150.0 + 25.0 * i as f64, -150.0 + 25.0 * j as f64));
        }
    }
    out
}

type Answer = Option<(i32, SimlinHitPart)>;

/// What the hit test answers at `(x, y)`, through the FFI.
unsafe fn hit_at(model: *mut SimlinModel, (x, y): (f64, f64)) -> Answer {
    let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
    let mut err = ptr::null_mut();
    simlin_model_hit_test(
        model, x, y, TOLERANCE, &mut hit, &mut uid, &mut part, &mut err,
    );
    expect_no_error(err, "a hit test");
    hit.then_some((uid, part))
}

/// What the hit test answers at every probe, through the FFI.
unsafe fn answers(model: *mut SimlinModel) -> Vec<Answer> {
    probes()
        .into_iter()
        .map(|point| hit_at(model, point))
        .collect()
}

/// What an index built now from the project's datamodel answers at every probe.
unsafe fn fresh_answers(proj: *mut SimlinProject) -> Vec<Answer> {
    let contents = (*proj).datamodel.lock().unwrap();
    let index = editing::HitIndex::new(&contents, "main").expect("main has a view");
    probes()
        .into_iter()
        .map(|(x, y)| {
            index
                .hit(Point::new(x, y), TOLERANCE)
                .map(|h| (h.uid, h.part.into()))
        })
        .collect()
}

unsafe fn apply(
    proj: *mut SimlinProject,
    patch: &Value,
    dry_run: bool,
    allow_errors: bool,
) -> *mut SimlinError {
    let bytes = serde_json::to_vec(patch).unwrap();
    let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
    simlin_project_apply_patch(
        proj,
        bytes.as_ptr(),
        bytes.len(),
        dry_run,
        allow_errors,
        &mut collected,
        &mut err,
    );
    if !collected.is_null() {
        simlin_error_free(collected);
    }
    err
}

fn edit_view(upsert: Value) -> Value {
    json!({"models": [{"name": "main", "ops": [{"type": "editView", "payload": {"index": 0, "upsert": [upsert], "remove": []}}]}]})
}

/// Moves the stock, which implies no model operation.
fn move_stock() -> Value {
    edit_view(json!({"type": "stock", "uid": STOCK, "name": "population", "x": 400.0, "y": 400.0}))
}

/// The entry points that mutate a project's datamodel.
#[derive(Clone, Copy, Debug)]
enum Mutation {
    /// `simlin_project_apply_patch` with a patch that implies no model
    /// operation, which applies without validating.
    ViewOnlyPatch,
    /// `simlin_project_apply_patch` with a patch that validates on a staged
    /// copy and commits it.
    StagedPatch,
    ReplaceContents,
    AddModel,
    DiagramSync,
    /// `simlin_tool_session_land_plan`: an agent's approved edit.
    #[cfg(feature = "agent_tools")]
    LandPlan,
}

impl Mutation {
    const ALL: &[Mutation] = &[
        Mutation::ViewOnlyPatch,
        Mutation::StagedPatch,
        Mutation::ReplaceContents,
        Mutation::AddModel,
        Mutation::DiagramSync,
        #[cfg(feature = "agent_tools")]
        Mutation::LandPlan,
    ];

    unsafe fn run(self, proj: *mut SimlinProject) {
        let mut err = ptr::null_mut();
        match self {
            Mutation::ViewOnlyPatch => err = apply(proj, &move_stock(), false, false),
            Mutation::StagedPatch => {
                // A new named element creates its variable, so the patch validates.
                let patch = edit_view(
                    json!({"type": "aux", "uid": 10, "name": "fresh", "x": 400.0, "y": 400.0}),
                );
                err = apply(proj, &patch, false, true);
            }
            Mutation::ReplaceContents => {
                let src = open(400.0, 400.0);
                simlin_project_replace_contents(proj, src, &mut err);
                simlin_project_unref(src);
            }
            Mutation::AddModel => {
                let name = CString::new("another").unwrap();
                simlin_project_add_model(proj, name.as_ptr(), &mut err);
            }
            Mutation::DiagramSync => {
                let name = CString::new("main").unwrap();
                simlin_project_diagram_sync(proj, name.as_ptr(), ptr::null(), &mut err);
            }
            #[cfg(feature = "agent_tools")]
            Mutation::LandPlan => {
                let model = main_model(proj);
                let session = planned_session(model);
                let id = CString::new("P1").unwrap();
                let (mut buf, mut len) = (ptr::null_mut(), 0);
                simlin_tool_session_land_plan(session, id.as_ptr(), &mut buf, &mut len, &mut err);
                let answer: Value = serde_json::from_slice(std::slice::from_raw_parts(buf, len))
                    .expect("a landing is JSON");
                assert_eq!(answer["landed"], true, "{answer}");
                simlin_free(buf);
                simlin_tool_session_unref(session);
                simlin_model_unref(model);
            }
        }
        expect_no_error(err, &format!("{self:?}"));
    }

    /// Whether the mutation changes what main's diagram draws.
    fn redraws_main(self) -> bool {
        match self {
            Mutation::ViewOnlyPatch
            | Mutation::StagedPatch
            | Mutation::ReplaceContents
            | Mutation::DiagramSync => true,
            #[cfg(feature = "agent_tools")]
            Mutation::LandPlan => true,
            Mutation::AddModel => false,
        }
    }
}

#[test]
fn every_mutating_entry_point_drops_the_hit_index_and_advances_the_revision() {
    let mut failures = Vec::new();
    for &mutation in Mutation::ALL {
        unsafe {
            let proj = open(100.0, 100.0);
            let model = main_model(proj);
            let before = answers(model);
            assert!(
                has_index(proj),
                "{mutation:?}: the hit test caches its index"
            );
            let revision_before = revision(proj);
            mutation.run(proj);
            if has_index(proj) {
                failures.push(format!("{mutation:?} kept the index"));
            }
            if revision(proj) <= revision_before {
                failures.push(format!("{mutation:?} did not advance the revision"));
            }
            let fresh = fresh_answers(proj);
            if answers(model) != fresh {
                failures.push(format!(
                    "{mutation:?}: the hit test answers what no index of the new datamodel answers"
                ));
            }
            // What makes the row able to catch a stale index at all: the new
            // datamodel answers somewhere what the old one did not.
            if mutation.redraws_main() && fresh == before {
                failures.push(format!(
                    "{mutation:?}: no probe tells the new diagram from the old, so the row cannot catch a stale index"
                ));
            }
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Entry points that only read a project's datamodel.
#[derive(Clone, Copy, Debug)]
enum Read {
    DryRunPatch,
    RejectedPatch,
    SimNew,
    GetErrors,
    RenderScene,
    SerializeJson,
    PlanTap,
    GestureBegin,
    PlanDelete,
    PlanRename,
    PlanMove,
    #[cfg(feature = "agent_tools")]
    ToolCall,
    #[cfg(feature = "agent_tools")]
    ToolChanges,
    #[cfg(feature = "agent_tools")]
    ToolRun,
    /// Planning an edit stages it on the db and restores; the plan is the
    /// host's to land.
    #[cfg(feature = "agent_tools")]
    ToolPlan,
}

/// A tool session over `model` that has read it and planned an edit, `P1`.
/// The calls run on a thread of its own, so one that waits on a lock it can
/// never take fails the test instead of hanging it.
#[cfg(feature = "agent_tools")]
unsafe fn planned_session(model: *mut SimlinModel) -> *mut crate::tools::SimlinToolSession {
    let model_addr = model as usize;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let session = plan_in(model_addr as *mut SimlinModel);
        let _ = tx.send(session as usize);
    });
    rx.recv_timeout(POSITIVE_WAIT)
        .expect("the session plans rather than waiting on a lock it can never take")
        as *mut crate::tools::SimlinToolSession
}

/// [`planned_session`]'s calls.
#[cfg(feature = "agent_tools")]
unsafe fn plan_in(model: *mut SimlinModel) -> *mut crate::tools::SimlinToolSession {
    let mut err = ptr::null_mut();
    let session = simlin_tool_session_new(model, &mut err);
    expect_no_error(err, "making a tool session");
    for (tool, input) in [
        ("read_model", "{}".to_string()),
        (
            "edit_model",
            json!({"summary": "a named growth rate", "operations": [
                {"op": "add_variable", "name": "base_rate", "equation": "0.1"},
                {"op": "set_equation", "variable": "rate", "equation": "base_rate"}
            ]})
            .to_string(),
        ),
    ] {
        let tool = CString::new(tool).unwrap();
        let (mut out, mut out_len, mut is_error) = (ptr::null_mut(), 0, false);
        simlin_tool_session_call(
            session,
            tool.as_ptr(),
            input.as_ptr(),
            input.len(),
            &mut out,
            &mut out_len,
            &mut is_error,
            &mut err,
        );
        expect_no_error(err, "a tool call");
        assert!(!is_error, "the call answers");
        simlin_free(out);
    }
    session
}

impl Read {
    const ALL: &[Read] = &[
        Read::DryRunPatch,
        Read::RejectedPatch,
        Read::SimNew,
        Read::GetErrors,
        Read::RenderScene,
        Read::SerializeJson,
        Read::PlanTap,
        Read::GestureBegin,
        Read::PlanDelete,
        Read::PlanRename,
        Read::PlanMove,
        #[cfg(feature = "agent_tools")]
        Read::ToolCall,
        #[cfg(feature = "agent_tools")]
        Read::ToolChanges,
        #[cfg(feature = "agent_tools")]
        Read::ToolRun,
        #[cfg(feature = "agent_tools")]
        Read::ToolPlan,
    ];

    unsafe fn run(self, proj: *mut SimlinProject, model: *mut SimlinModel) {
        let main = CString::new("main").unwrap();
        let mut err = ptr::null_mut();
        let (mut buf, mut len) = (ptr::null_mut(), 0);
        let press = SimlinPress {
            x: 100.0,
            y: 100.0,
            has_hit: true,
            hit_uid: STOCK,
            hit_part: SimlinHitPart::Body,
            tool: SimlinTool::None,
            selection: ptr::null(),
            selection_len: 0,
            toggle: false,
            pointer: SimlinPointerKind::Mouse,
            target_slop: TOLERANCE,
        };
        match self {
            Read::DryRunPatch => err = apply(proj, &move_stock(), true, false),
            Read::RejectedPatch => {
                let patch = json!({"models": [{"name": "main", "ops": [{"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "rate +"}}}]}]});
                let rejected = apply(proj, &patch, false, false);
                assert!(
                    !rejected.is_null(),
                    "a patch with an equation error is rejected"
                );
                simlin_error_free(rejected);
            }
            Read::SimNew => {
                let sim = simlin_sim_new(model, false, &mut err);
                simlin_sim_unref(sim);
            }
            Read::GetErrors => {
                let errors = simlin_project_get_errors(proj, &mut err);
                if !errors.is_null() {
                    simlin_error_free(errors);
                }
            }
            Read::RenderScene => {
                simlin_project_render_scene(proj, main.as_ptr(), &mut buf, &mut len, &mut err)
            }
            Read::SerializeJson => {
                simlin_project_serialize_json(proj, 0, false, &mut buf, &mut len, &mut err)
            }
            Read::PlanTap => simlin_model_plan_tap(model, &press, &mut buf, &mut len, &mut err),
            Read::GestureBegin => {
                simlin_gesture_unref(simlin_gesture_begin(model, &press, &mut err))
            }
            Read::PlanDelete => {
                let selection = [STOCK];
                simlin_model_plan_delete(
                    model,
                    selection.as_ptr(),
                    1,
                    &mut buf,
                    &mut len,
                    &mut err,
                );
            }
            Read::PlanRename => {
                let (from, to) = (
                    CString::new("rate").unwrap(),
                    CString::new("growth").unwrap(),
                );
                simlin_model_plan_rename(
                    model,
                    from.as_ptr(),
                    to.as_ptr(),
                    &mut buf,
                    &mut len,
                    &mut err,
                );
            }
            Read::PlanMove => {
                let selection = [STOCK];
                simlin_model_plan_move(
                    model,
                    selection.as_ptr(),
                    1,
                    10.0,
                    0.0,
                    &mut buf,
                    &mut len,
                    &mut err,
                );
            }
            #[cfg(feature = "agent_tools")]
            Read::ToolRun => {
                let session = simlin_tool_session_new(model, &mut err);
                expect_no_error(err, "making a tool session");
                err = ptr::null_mut();
                let current = CString::new("current").unwrap();
                let results = simlin_tool_session_get_run(
                    session,
                    current.as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut err,
                );
                simlin_results_unref(results);
                simlin_tool_session_unref(session);
            }
            #[cfg(feature = "agent_tools")]
            Read::ToolPlan => simlin_tool_session_unref(planned_session(model)),
            #[cfg(feature = "agent_tools")]
            Read::ToolCall | Read::ToolChanges => {
                let session = simlin_tool_session_new(model, &mut err);
                expect_no_error(err, "making a tool session");
                err = ptr::null_mut();
                if matches!(self, Read::ToolCall) {
                    let tool = CString::new("read_model").unwrap();
                    let mut is_error = false;
                    simlin_tool_session_call(
                        session,
                        tool.as_ptr(),
                        ptr::null(),
                        0,
                        &mut buf,
                        &mut len,
                        &mut is_error,
                        &mut err,
                    );
                    assert!(!is_error, "read_model answers");
                } else {
                    simlin_tool_session_get_changes(session, &mut buf, &mut len, &mut err);
                }
                simlin_tool_session_unref(session);
            }
        }
        expect_no_error(err, &format!("{self:?}"));
        if !buf.is_null() {
            simlin_free(buf);
        }
    }
}

#[test]
fn an_entry_point_that_reads_keeps_the_hit_index_and_the_revision() {
    let mut failures = Vec::new();
    for read in Read::ALL {
        unsafe {
            let proj = open(100.0, 100.0);
            let model = main_model(proj);
            let before = answers(model);
            let revision_before = revision(proj);
            read.run(proj, model);
            if !has_index(proj) {
                failures.push(format!("{read:?} dropped the index"));
            }
            if revision(proj) != revision_before {
                failures.push(format!("{read:?} advanced the revision"));
            }
            if answers(model) != before {
                failures.push(format!("{read:?} changed what the diagram answers"));
            }
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A copy of `proj`, made as a host copies a project: its contents replaced
/// with `proj`'s in a new project.
unsafe fn copy_of(proj: *mut SimlinProject) -> *mut SimlinProject {
    let mut err = ptr::null_mut();
    let copy = simlin_project_new(ptr::null(), &mut err);
    expect_no_error(err, "a new project");
    simlin_project_replace_contents(copy, proj, &mut err);
    expect_no_error(err, "copying the project");
    copy
}

unsafe fn shares_datamodel(a: *mut SimlinProject, b: *mut SimlinProject) -> bool {
    let a = (*a).datamodel.lock().unwrap().shared();
    let b = (*b).datamodel.lock().unwrap().shared();
    Arc::ptr_eq(&a, &b)
}

unsafe fn datamodel_of(proj: *mut SimlinProject) -> simlin_engine::datamodel::Project {
    (**(*proj).datamodel.lock().unwrap()).clone()
}

/// A copy shares the datamodel until either side is edited, and keeps a
/// revision of its own: an edit of one side advances that side's alone.
#[test]
fn a_copy_shares_the_datamodel_until_an_edit_of_either_side_copies_it() {
    let mut failures = Vec::new();
    for mutation in Mutation::ALL {
        for edit_the_copy in [true, false] {
            let side = if edit_the_copy {
                "the copy"
            } else {
                "the original"
            };
            unsafe {
                let original = open(100.0, 100.0);
                let copy = copy_of(original);
                if !shares_datamodel(original, copy) {
                    failures.push(format!("{mutation:?}: a copy holds a datamodel of its own"));
                }
                let before = datamodel_of(original);
                let (edited, other) = if edit_the_copy {
                    (copy, original)
                } else {
                    (original, copy)
                };
                let (edited_revision, other_revision) = (revision(edited), revision(other));
                mutation.run(edited);
                if revision(edited) <= edited_revision {
                    failures.push(format!("{mutation:?} of {side} left its revision"));
                }
                if revision(other) != other_revision {
                    failures.push(format!(
                        "{mutation:?} of {side} advanced the other's revision"
                    ));
                }
                if datamodel_of(other) != before {
                    failures.push(format!("{mutation:?} of {side} reached the other side"));
                }
                // What makes the row able to catch an edit reaching the other
                // side at all: the edit changed the side it was made on.
                if datamodel_of(edited) == before {
                    failures.push(format!("{mutation:?} of {side} changed nothing"));
                }
                if shares_datamodel(original, copy) {
                    failures.push(format!("{mutation:?} of {side} left both sides sharing"));
                }
                simlin_project_unref(copy);
                simlin_project_unref(original);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_dry_run_or_a_rejected_patch_leaves_a_copy_sharing_the_datamodel() {
    for read in [Read::DryRunPatch, Read::RejectedPatch] {
        unsafe {
            let original = open(100.0, 100.0);
            let copy = copy_of(original);
            let model = main_model(copy);
            read.run(copy, model);
            assert!(
                shares_datamodel(original, copy),
                "{read:?} copied a datamodel it did not change"
            );
            simlin_model_unref(model);
            simlin_project_unref(copy);
            simlin_project_unref(original);
        }
    }
}

#[test]
fn a_copy_outlives_the_project_it_was_copied_from() {
    unsafe {
        let original = open(100.0, 100.0);
        let before = datamodel_of(original);
        let copy = copy_of(original);
        simlin_project_unref(original);
        assert!(datamodel_of(copy) == before);
        simlin_project_unref(copy);
    }
}

#[test]
fn a_mutable_borrow_drops_every_index_and_a_shared_borrow_keeps_them() {
    unsafe {
        let proj = open(100.0, 100.0);
        let mut contents = (*proj).datamodel.lock().unwrap();
        let other = {
            let mut model = contents.get_model("main").unwrap().clone();
            model.name = "other".to_string();
            model
        };
        contents.models.push(other);
        for name in ["main", "other"] {
            contents.hit_index(name).expect("the model has a view");
        }
        let revision = contents.revision();
        let _: &simlin_engine::datamodel::Project = &contents;
        assert!(contents.has_hit_index("main") && contents.has_hit_index("other"));
        assert_eq!(
            contents.revision(),
            revision,
            "a shared borrow keeps the revision"
        );
        let _: &mut simlin_engine::datamodel::Project = &mut contents;
        assert!(!contents.has_hit_index("main") && !contents.has_hit_index("other"));
        assert_eq!(
            contents.revision(),
            revision + 1,
            "a mutable borrow advances the revision once"
        );
        drop(contents);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_project_opens_at_revision_zero_and_the_revision_refuses_a_null_out_pointer() {
    unsafe {
        let proj = open(100.0, 100.0);
        assert_eq!(revision(proj), 0);
        let mut err = ptr::null_mut();
        simlin_project_get_revision(proj, ptr::null_mut(), &mut err);
        assert!(!err.is_null(), "a NULL out pointer is refused");
        simlin_error_free(err);
        let mut revision = 7;
        let mut err = ptr::null_mut();
        simlin_project_get_revision(ptr::null_mut(), &mut revision, &mut err);
        assert!(!err.is_null(), "a NULL project is refused");
        assert_eq!(revision, 7, "a refused read writes nothing");
        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_view_that_does_not_resolve_caches_nothing() {
    unsafe {
        let proj = open(100.0, 100.0);
        let mut contents = (*proj).datamodel.lock().unwrap();
        assert!(contents.hit_index("missing").is_err());
        assert!(!contents.has_hit_index("missing"));
        drop(contents);
        simlin_project_unref(proj);
    }
}

/// A positive wait ("the other thread should get here"), generous for the
/// reason `tests_concurrency.rs` gives: `recv_timeout` returns as soon as the
/// message arrives, so the budget is spent only on a genuine failure.
const POSITIVE_WAIT: Duration = Duration::from_secs(30);

/// A negative wait ("the other thread should not have answered yet"), which
/// slowness can only make pass.
const NEGATIVE_WAIT: Duration = Duration::from_millis(200);

/// The tap planned for a press at `(x, y)` carrying `hit`, as JSON.
unsafe fn tap_plan(model: *mut SimlinModel, (x, y): (f64, f64), hit: Answer) -> Value {
    let press = SimlinPress {
        x,
        y,
        has_hit: hit.is_some(),
        hit_uid: hit.map_or(0, |(uid, _)| uid),
        hit_part: hit.map_or(SimlinHitPart::Body, |(_, part)| part),
        tool: SimlinTool::None,
        selection: ptr::null(),
        selection_len: 0,
        toggle: false,
        pointer: SimlinPointerKind::Mouse,
        target_slop: TOLERANCE,
    };
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_model_plan_tap(model, &press, &mut buf, &mut len, &mut err);
    expect_no_error(err, "planning a tap");
    let plan =
        serde_json::from_slice(std::slice::from_raw_parts(buf, len)).expect("a plan is JSON");
    simlin_free(buf);
    plan
}

#[test]
fn a_hit_test_issued_while_an_edit_holds_the_datamodel_waits_and_answers_the_contents_the_edit_leaves_as_the_planners_do(
) {
    use crate::patch::{install_patch_test_hook, PatchHookPoint};
    const FRESH: i32 = 10;
    let point = (400.0, 400.0);
    unsafe {
        let proj = open(100.0, 100.0);
        let model = main_model(proj);
        // Warms the index, so a hit test that answered from it without the
        // datamodel's lock would answer, from the contents before the edit,
        // while the edit holds the project.
        assert_eq!(
            hit_at(model, point),
            None,
            "nothing is drawn where the edit creates its aux"
        );

        let proj_addr = proj as usize;
        let (staging_tx, staging_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let hook = Arc::new(move |at: PatchHookPoint, project: &SimlinProject| {
            if at == PatchHookPoint::StagedSyncWhileDbLocked
                && (project as *const SimlinProject as usize) == proj_addr
            {
                let _ = staging_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }
        });
        let _hook_guard = install_patch_test_hook(hook);

        // A new named element creates its variable, so the edit validates on a
        // staged copy, holding both project locks where the hook waits.
        let patch = edit_view(
            json!({"type": "aux", "uid": FRESH, "name": "fresh", "x": point.0, "y": point.1}),
        );
        let writer = thread::spawn(move || {
            let err = apply(proj_addr as *mut SimlinProject, &patch, false, true);
            expect_no_error(err, "the edit");
        });
        staging_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the edit reaches staging");

        // A host's press: a hit test, then the tap planned from its hit.
        let model_addr = model as usize;
        let (hit_tx, hit_rx) = mpsc::channel();
        let (plan_tx, plan_rx) = mpsc::channel();
        let press = thread::spawn(move || {
            let model = model_addr as *mut SimlinModel;
            let hit = hit_at(model, point);
            let _ = hit_tx.send(hit);
            let _ = plan_tx.send(tap_plan(model, point, hit));
        });
        assert!(
            hit_rx.recv_timeout(NEGATIVE_WAIT).is_err(),
            "a hit test waits while an edit holds the datamodel"
        );
        release_tx.send(()).unwrap();
        writer.join().unwrap();

        let hit = hit_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the hit test answers once the edit lands");
        assert_eq!(
            hit,
            Some((FRESH, SimlinHitPart::Body)),
            "the hit test answers the contents the edit leaves"
        );
        let plan = plan_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the tap is planned");
        assert_eq!(
            plan["selection"],
            json!([FRESH]),
            "the tap planned from that hit selects what the edit created"
        );
        press.join().unwrap();
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
