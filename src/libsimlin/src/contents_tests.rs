// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A published hit index is never older than the datamodel it answers for, and
//! a hit test with a current index never waits for the datamodel's lock.
//!
//! Every mutable borrow of the datamodel advances the project's revision
//! (`ProjectContents`'s `DerefMut`), so no entry point can mutate without making
//! the published indexes stale. The rows pin it for the entry points that mutate
//! a project's datamodel, found among its lock sites
//! (`grep -n 'datamodel.lock()' src`): `simlin_project_apply_patch`, through its
//! view-only apply and its staged commit, `simlin_project_replace_contents`,
//! `simlin_project_add_model` and `simlin_project_diagram_sync`; every other site
//! reads. Each row warms the index, mutates through the entry point, and requires
//! that the published index is no longer current and that the hit test answers at
//! every probe point what an index built from the new datamodel answers. For
//! every row but `add_model`, which changes no model's view, the old index
//! answered differently somewhere, so a stale index fails the row.
//!
//! The reads keep the published index current, pinned for a row per kind of
//! read: a dry-run and a rejected patch, a simulation, diagnostics, a scene,
//! serialization, and the editing planners. A read that made the index stale
//! would only cost a rebuild, so these rows pin performance, not freshness.
//!
//! The concurrency tests hold the project on other threads the way an edit
//! does. While another thread holds the datamodel's lock, a hit test with a
//! current index answers from it, and one whose index is stale waits and answers
//! from the datamodel as the holder left it. While an edit stages under both
//! project locks, a hit test answers from the committed contents, and once the
//! commit lands it answers from the new contents.

use std::ffi::CString;
use std::ptr;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use simlin_engine::datamodel::{self, ViewElement};
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

/// Whether a current hit index of main is published.
unsafe fn has_index(proj: *mut SimlinProject) -> bool {
    (*proj).published.hit_index("main").is_some()
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

/// What the hit test answers at every probe, through the FFI.
unsafe fn answers(model: *mut SimlinModel) -> Vec<Answer> {
    probes()
        .into_iter()
        .map(|(x, y)| {
            let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
            let mut err = ptr::null_mut();
            simlin_model_hit_test(
                model, x, y, TOLERANCE, &mut hit, &mut uid, &mut part, &mut err,
            );
            expect_no_error(err, "a hit test");
            hit.then_some((uid, part))
        })
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
}

impl Mutation {
    const ALL: [Mutation; 5] = [
        Mutation::ViewOnlyPatch,
        Mutation::StagedPatch,
        Mutation::ReplaceContents,
        Mutation::AddModel,
        Mutation::DiagramSync,
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
            Mutation::AddModel => false,
        }
    }
}

#[test]
fn every_mutating_entry_point_makes_the_published_hit_index_stale() {
    let mut failures = Vec::new();
    for mutation in Mutation::ALL {
        unsafe {
            let proj = open(100.0, 100.0);
            let model = main_model(proj);
            let before = answers(model);
            assert!(
                has_index(proj),
                "{mutation:?}: the hit test publishes its index"
            );
            mutation.run(proj);
            if has_index(proj) {
                failures.push(format!("{mutation:?} left the published index current"));
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
}

impl Read {
    const ALL: [Read; 11] = [
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
        }
        expect_no_error(err, &format!("{self:?}"));
        if !buf.is_null() {
            simlin_free(buf);
        }
    }
}

#[test]
fn an_entry_point_that_reads_keeps_the_published_hit_index_current() {
    let mut failures = Vec::new();
    for read in Read::ALL {
        unsafe {
            let proj = open(100.0, 100.0);
            let model = main_model(proj);
            let before = answers(model);
            read.run(proj, model);
            if !has_index(proj) {
                failures.push(format!("{read:?} made the published index stale"));
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

#[test]
fn a_mutable_borrow_makes_every_published_index_stale_and_a_shared_borrow_keeps_them() {
    unsafe {
        let proj = open(100.0, 100.0);
        let published = &(*proj).published;
        let mut contents = (*proj).datamodel.lock().unwrap();
        let other = {
            let mut model = contents.get_model("main").unwrap().clone();
            model.name = "other".to_string();
            model
        };
        contents.models.push(other);
        for name in ["main", "other"] {
            contents
                .publish_hit_index(name)
                .expect("the model has a view");
        }
        let _: &datamodel::Project = &contents;
        assert!(published.hit_index("main").is_some() && published.hit_index("other").is_some());
        let _: &mut datamodel::Project = &mut contents;
        assert!(published.hit_index("main").is_none() && published.hit_index("other").is_none());
        drop(contents);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_view_that_does_not_resolve_publishes_nothing() {
    unsafe {
        let proj = open(100.0, 100.0);
        let contents = (*proj).datamodel.lock().unwrap();
        assert!(contents.publish_hit_index("missing").is_err());
        assert!((*proj).published.hit_index("missing").is_none());
        drop(contents);
        simlin_project_unref(proj);
    }
}

/// A positive wait ("the other thread should get here"), generous for the
/// reason `tests_concurrency.rs` gives: `recv_timeout` returns as soon as the
/// message arrives, so the budget is spent only on a genuine failure.
const POSITIVE_WAIT: Duration = Duration::from_secs(30);

/// A negative wait ("the other thread should not have finished yet"), which
/// slowness can only make pass.
const NEGATIVE_WAIT: Duration = Duration::from_millis(200);

/// Moves main's stock to `(x, y)` on its first view.
fn move_the_stock(project: &mut datamodel::Project, x: f64, y: f64) {
    let model = project
        .models
        .iter_mut()
        .find(|m| m.name == "main")
        .expect("the model main");
    let Some(datamodel::View::StockFlow(view)) = model.views.first_mut() else {
        panic!("main has a view");
    };
    let Some(ViewElement::Stock(stock)) = view.elements.iter_mut().find(|e| e.get_uid() == STOCK)
    else {
        panic!("main draws the stock");
    };
    stock.x = x;
    stock.y = y;
}

/// Locks `proj`'s datamodel on another thread, lets `change` see the contents
/// (a mutable borrow of the datamodel in it advances the revision), and holds
/// the lock until the returned sender sends or is dropped. Returns once the lock
/// is held.
unsafe fn hold_datamodel(
    proj: *mut SimlinProject,
    change: impl FnOnce(&mut ProjectContents) + Send + 'static,
) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let addr = proj as usize;
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let proj = addr as *mut SimlinProject;
        let mut contents = unsafe { (*proj).datamodel.lock().unwrap() };
        change(&mut contents);
        held_tx.send(()).expect("the test waits for the lock");
        let _ = release_rx.recv();
        drop(contents);
    });
    held_rx
        .recv_timeout(POSITIVE_WAIT)
        .expect("the holder locks the datamodel");
    (release_tx, holder)
}

/// What the hit test answers at every probe, asked on another thread and sent
/// once every answer is in.
unsafe fn answer_on_another_thread(
    model: *mut SimlinModel,
) -> (mpsc::Receiver<Vec<Answer>>, thread::JoinHandle<()>) {
    let addr = model as usize;
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let answered = unsafe { answers(addr as *mut SimlinModel) };
        let _ = tx.send(answered);
    });
    (rx, reader)
}

#[test]
fn a_warm_hit_test_answers_while_another_thread_holds_the_datamodel() {
    unsafe {
        let proj = open(100.0, 100.0);
        let model = main_model(proj);
        let before = answers(model);
        // The holder borrows nothing mutably, so the published index stays current.
        let (release, holder) = hold_datamodel(proj, |_| {});
        let (answered, reader) = answer_on_another_thread(model);
        let during = answered
            .recv_timeout(POSITIVE_WAIT)
            .expect("a warm hit test answers without waiting for the datamodel's lock");
        assert_eq!(during, before);
        release.send(()).unwrap();
        holder.join().unwrap();
        reader.join().unwrap();
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_cold_hit_test_waits_for_the_datamodel_and_answers_what_it_holds() {
    unsafe {
        let proj = open(100.0, 100.0);
        let model = main_model(proj);
        let before = answers(model);
        let (release, holder) =
            hold_datamodel(proj, |contents| move_the_stock(contents, 400.0, 400.0));
        let (answered, reader) = answer_on_another_thread(model);
        assert!(
            answered.recv_timeout(NEGATIVE_WAIT).is_err(),
            "a hit test with no current index waits while the datamodel is locked"
        );
        release.send(()).unwrap();
        holder.join().unwrap();
        let after = answered
            .recv_timeout(POSITIVE_WAIT)
            .expect("the hit test answers once the datamodel is free");
        reader.join().unwrap();
        let fresh = fresh_answers(proj);
        assert_eq!(
            after, fresh,
            "it answers from the datamodel as the holder left it"
        );
        assert_ne!(
            fresh, before,
            "the probes tell the moved stock from where it was"
        );
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_hit_test_during_a_staged_edit_answers_the_committed_contents_until_the_commit_lands() {
    use crate::patch::{install_patch_test_hook, PatchHookPoint};
    unsafe {
        let proj = open(100.0, 100.0);
        let model = main_model(proj);
        let before = answers(model);
        let proj_addr = proj as usize;
        let (staging_tx, staging_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let hook = Arc::new(move |point: PatchHookPoint, project: &SimlinProject| {
            if point == PatchHookPoint::StagedSyncWhileDbLocked
                && (project as *const SimlinProject as usize) == proj_addr
            {
                let _ = staging_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }
        });
        let _hook_guard = install_patch_test_hook(hook);

        // Moves the stock and creates an aux, whose variable makes the edit
        // validate on a staged copy under both project locks.
        let patch = json!({"models": [{"name": "main", "ops": [{"type": "editView", "payload": {"index": 0, "upsert": [
            {"type": "stock", "uid": STOCK, "name": "population", "x": 400.0, "y": 400.0},
            {"type": "aux", "uid": 10, "name": "fresh", "x": 400.0, "y": 600.0}
        ], "remove": []}}]}]});
        let writer = thread::spawn(move || {
            let err = apply(proj_addr as *mut SimlinProject, &patch, false, true);
            expect_no_error(err, "the staged edit");
        });
        staging_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the edit reaches staging");

        let (answered, reader) = answer_on_another_thread(model);
        let during = answered
            .recv_timeout(POSITIVE_WAIT)
            .expect("a hit test answers while the edit stages");
        assert_eq!(
            during, before,
            "while the edit stages, a hit test answers from the committed contents"
        );
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        reader.join().unwrap();

        let after = answers(model);
        assert_eq!(
            after,
            fresh_answers(proj),
            "once the commit lands, a hit test answers from the new contents"
        );
        assert_ne!(
            after, before,
            "the probes tell the new contents from the committed ones"
        );
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
