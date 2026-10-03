// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The hit index a project caches is never older than its datamodel, its
//! revision advances with every change and with nothing else, and a copy
//! shares the datamodel until either side is edited.
//!
//! Every mutable borrow of the datamodel drops the cached indexes, advances
//! the revision and copies a shared datamodel first (`ProjectContents`'s
//! `DerefMut`), so no entry point can mutate without invalidating, without
//! counting the change, or into another project. Which entry points change
//! the contents is a column of the entry point table
//! (`entry_point_tests.rs`), and the table's rules hold each row to it: a
//! row that writes drops the index, advances the revision, answers at every
//! point what an index built from the new datamodel answers (and, where it
//! redraws the diagram, differently somewhere than the old index did), and
//! leaves a copy as it was; every other row keeps the index, the revision
//! and the sharing. A read that dropped the index would only cost a rebuild,
//! but a read that advanced the revision would tell every host caching
//! against it that the project had changed when it had not.
//!
//! The rules here are `ProjectContents`'s own, and the one contract between
//! two entry points: a hit test locks the datamodel as the planners do, so a
//! press issued while an edit holds the project waits for the edit to land.
//! The contract test holds an edit at its staging point, under both project
//! locks, and presses from another thread where the edit creates an aux: the
//! hit test answers nothing while the edit holds the project, then answers
//! the aux, and the tap planned from that hit selects it.

use std::ptr;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::entry_point_tests::{
    apply_patch, copy_of, datamodel_of, edit_view, expect_no_error, hit_at, main_model, open,
    revision, Hit, NEGATIVE_WAIT, POSITIVE_WAIT, TOLERANCE,
};
use crate::*;

#[test]
fn a_copy_outlives_the_project_it_was_copied_from() {
    unsafe {
        let original = open(true);
        let before = datamodel_of(original);
        let copy = copy_of(original);
        simlin_project_unref(original);
        assert!(*datamodel_of(copy) == *before);
        simlin_project_unref(copy);
    }
}

#[test]
fn a_mutable_borrow_drops_every_index_and_a_shared_borrow_keeps_them() {
    unsafe {
        let proj = open(true);
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

/// A replacement by the datamodel the contents hold already is no change:
/// the revision and the indexes stay. A replacement by another datamodel,
/// equal or not, drops the indexes and advances the revision.
#[test]
fn a_replacement_by_the_datamodel_it_holds_changes_nothing() {
    unsafe {
        let proj = open(true);
        let mut contents = (*proj).datamodel.lock().unwrap();
        contents.hit_index("main").expect("main has a view");
        let revision = contents.revision();

        let held = contents.shared();
        assert!(contents.holds(&held));
        contents.replace(held);
        assert_eq!(contents.revision(), revision);
        assert!(contents.has_hit_index("main"));

        let equal = Arc::new((**contents).clone());
        assert!(!contents.holds(&equal));
        contents.replace(equal);
        assert_eq!(contents.revision(), revision + 1);
        assert!(!contents.has_hit_index("main"));
        drop(contents);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_project_opens_at_revision_zero_and_the_revision_refuses_a_null_out_pointer() {
    unsafe {
        let proj = open(true);
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
        let proj = open(true);
        let mut contents = (*proj).datamodel.lock().unwrap();
        assert!(contents.hit_index("missing").is_err());
        assert!(!contents.has_hit_index("missing"));
        drop(contents);
        simlin_project_unref(proj);
    }
}

/// The tap planned for a press at `(x, y)` carrying `hit`, as JSON.
unsafe fn tap_plan(model: *mut SimlinModel, (x, y): (f64, f64), hit: Hit) -> Value {
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
        let proj = open(true);
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
            let err = apply_patch(proj_addr as *mut SimlinProject, &patch, false, true);
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
