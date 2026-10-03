// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A project builds its salsa database the first time an entry point runs a
//! query, and never before.
//!
//! Which entry points query, and so build one, is a column of the entry
//! point table (`entry_point_tests.rs`): a row per entry point runs first on
//! a project that has no database and requires the answer a project whose
//! database was built at open gives, and that everything else -- an open, a
//! copy, a read, a serialization, a drawing, an edit of the diagram -- builds
//! nothing. The rules here are about one entry point each: a copy compiles
//! only when queried, an added model reaches a database that exists, a query
//! takes the database alone once it exists and waits for the datamodel to
//! build it, the release of a superseded compile, and what a new project is.

use std::ffi::CString;
use std::ptr;
use std::sync::{mpsc, Arc};
use std::thread;

use serde_json::json;

use crate::entry_point_tests::{
    copy_of, datamodel_of, expect_no_error, main_model, open, take_error, EntryPoint, ALL,
    NEGATIVE_WAIT, POSITIVE_WAIT,
};
use crate::*;

/// The table's row for `name`, the one use of it.
fn entry_point(name: &str) -> &'static EntryPoint {
    let mut rows = ALL.iter().filter(|row| row.name == name);
    let row = rows.next().expect("the entry point has a row");
    assert!(rows.next().is_none(), "{name} has one row");
    row
}

#[test]
fn a_new_project_and_a_copy_build_no_database() {
    unsafe {
        let mut err = ptr::null_mut();
        let new = simlin_project_new(ptr::null(), &mut err);
        expect_no_error(err, "a new project");
        assert!(!(*new).has_db(), "a new project built a database");
        simlin_project_unref(new);

        let proj = open(true);
        let copy = copy_of(proj);
        assert!(!(*proj).has_db(), "copying built a database");
        assert!(!(*copy).has_db(), "copying built the copy a database");
        simlin_project_unref(copy);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_copy_of_a_compiled_project_builds_a_database_of_its_own_only_when_queried() {
    let sim_new = entry_point("simlin_sim_new");
    unsafe {
        let original = open(true);
        let expected = sim_new.run_plainly(original);
        let copy = copy_of(original);
        assert!(
            !(*copy).has_db(),
            "a copy of a compiled project is not compiled"
        );
        // An edit of the original re-syncs the original's database only.
        let patch = serde_json::to_vec(&json!({"models": [{"name": "main", "ops": [
            {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "0.2"}}}
        ]}]}))
        .unwrap();
        let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
        simlin_project_apply_patch(
            original,
            patch.as_ptr(),
            patch.len(),
            false,
            false,
            &mut collected,
            &mut err,
        );
        expect_no_error(collected, "collecting the edit's errors");
        expect_no_error(err, "the edit");
        assert!(
            !(*copy).has_db(),
            "an edit of the original builds the copy nothing"
        );
        assert_ne!(
            sim_new.run_plainly(original),
            expected,
            "the edit changes what the original simulates"
        );
        assert_eq!(
            sim_new.run_plainly(copy),
            expected,
            "the copy simulates the contents it was copied with"
        );
        assert!((*copy).has_db());
        simlin_project_unref(copy);
        simlin_project_unref(original);
    }
}

#[test]
fn a_model_added_to_a_project_with_a_database_reaches_the_database() {
    unsafe {
        let proj = open(true);
        drop((*proj).lock_db());
        let another = CString::new("another").unwrap();
        let mut err = ptr::null_mut();
        simlin_project_add_model(proj, another.as_ptr(), &mut err);
        expect_no_error(err, "adding a model");
        // A model the database has never seen has no sync state to compile,
        // so its simulation would fail to run.
        let model = simlin_project_get_model(proj, another.as_ptr(), &mut err);
        expect_no_error(err, "getting the added model");
        let sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "creating the added model's simulation");
        simlin_sim_run_to_end(sim, &mut err);
        expect_no_error(err, "running the added model");
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn once_built_a_query_answers_while_another_thread_holds_the_datamodel() {
    let latex = entry_point("simlin_model_get_latex_equation");
    unsafe {
        let proj = open(true);
        drop((*proj).lock_db());
        let expected = latex.run_plainly(proj);
        let model = main_model(proj);
        let held = (*proj).datamodel.lock().unwrap();
        let model_addr = model as usize;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // Through a model handle already in hand, so the query takes no
            // lock but its own.
            let births = CString::new("births").unwrap();
            let mut err = ptr::null_mut();
            let latex = simlin_model_get_latex_equation(
                model_addr as *mut SimlinModel,
                births.as_ptr(),
                &mut err,
            );
            let answer = std::ffi::CStr::from_ptr(latex)
                .to_string_lossy()
                .into_owned();
            simlin_free_string(latex);
            let _ = tx.send(format!("{answer} / {:?}", take_error(err)));
        });
        let answer = rx.recv_timeout(POSITIVE_WAIT).ok();
        drop(held);
        simlin_model_unref(model);
        assert_eq!(
            answer.as_deref(),
            Some(expected.as_str()),
            "a query waited on the datamodel after the database was built"
        );
        simlin_project_unref(proj);
    }
}

#[test]
fn a_first_query_waits_for_the_datamodel_and_builds_from_the_contents_it_is_released_with() {
    let latex = entry_point("simlin_model_get_latex_equation");
    unsafe {
        let proj = open(true);
        let unedited = open(true);
        let before = latex.run_plainly(unedited);
        simlin_project_unref(unedited);
        let mut held = (*proj).datamodel.lock().unwrap();
        let project: &mut simlin_engine::datamodel::Project = &mut held;
        let births = project.models[0]
            .variables
            .find_mut(|v| v.get_ident() == "births")
            .expect("births is a variable");
        if let simlin_engine::datamodel::Variable::Flow(flow) = births {
            flow.equation =
                simlin_engine::datamodel::Equation::Scalar("population * rate * 2".to_string());
        }
        let proj_addr = proj as usize;
        let (tx, rx) = mpsc::channel();
        let query = thread::spawn(move || {
            let answer = latex.run_plainly(proj_addr as *mut SimlinProject);
            let _ = tx.send(answer);
        });
        assert!(
            rx.recv_timeout(NEGATIVE_WAIT).is_err(),
            "a first query waits while the datamodel is held"
        );
        drop(held);
        let answer = rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the query answers once the datamodel is released");
        query.join().unwrap();
        assert_ne!(
            answer, before,
            "the database is built from the datamodel as the holder left it"
        );
        assert!((*proj).has_db());
        simlin_project_unref(proj);
    }
}

/// The lock that hands out the database releases, when it drops, the memos
/// the entry point's queries superseded (`DbLock`, `SimlinDb::
/// release_replaced_memos`): 117 MiB on C-LEARN under LTM after one rename,
/// which otherwise stays resident until the next edit. A simulation shares
/// its compiled program with the salsa memo that assembled it, so the
/// program a later compile supersedes is freed exactly when the release
/// runs, and a weak reference to it says whether it has.
#[test]
fn a_superseded_compiled_program_is_freed_by_the_entry_point_that_replaced_it() {
    unsafe {
        let proj = open(true);
        let model = main_model(proj);
        let mut err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "creating a simulation");
        let superseded = Arc::downgrade(
            (*sim)
                .state
                .lock()
                .unwrap()
                .compiled
                .as_ref()
                .expect("the model compiles"),
        );
        simlin_sim_unref(sim);
        assert!(
            superseded.upgrade().is_some(),
            "the database's memo keeps the program"
        );

        let patch = serde_json::to_vec(&json!({"models": [{"name": "main", "ops": [
            {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "0.3"}}}
        ]}]}))
        .unwrap();
        let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
        simlin_project_apply_patch(
            proj,
            patch.as_ptr(),
            patch.len(),
            false,
            false,
            &mut collected,
            &mut err,
        );
        expect_no_error(collected, "collecting the edit's errors");
        expect_no_error(err, "the edit");
        let sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "creating a simulation of the edited model");
        let freed = superseded.upgrade().is_none();
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
        assert!(
            freed,
            "the program the edit superseded stays resident after the compile that replaced it"
        );
    }
}

#[test]
fn a_new_project_is_the_empty_project_a_modeler_starts_from() {
    unsafe {
        for name in [Some("untitled"), None] {
            let c_name = name.map(|n| CString::new(n).unwrap());
            let mut err = ptr::null_mut();
            let new = simlin_project_new(
                c_name.as_ref().map_or(ptr::null(), |n| n.as_ptr()),
                &mut err,
            );
            expect_no_error(err, "a new project");
            // The empty project the server creates for a new model
            // (src/server/project-creation.ts).
            let json = serde_json::to_vec(&json!({
                "name": name.unwrap_or(""),
                "simSpecs": {"startTime": 0, "endTime": 100, "dt": "1"},
                "models": [{
                    "name": "main",
                    "stocks": [],
                    "flows": [],
                    "auxiliaries": [],
                    "views": [{"kind": "stock_flow", "elements": []}]
                }]
            }))
            .unwrap();
            let opened = simlin_project_open_json(json.as_ptr(), json.len(), 0, &mut err);
            expect_no_error(err, "opening the empty project");
            assert!(
                *datamodel_of(new) == *datamodel_of(opened),
                "a new project named {name:?} is not the empty project"
            );
            // Its view is one the editing entry points accept.
            let model = main_model(new);
            let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
            simlin_model_hit_test(
                model, 0.0, 0.0, 6.0, &mut hit, &mut uid, &mut part, &mut err,
            );
            expect_no_error(err, "a hit test on a new project");
            assert!(!hit);
            simlin_model_unref(model);
            simlin_project_unref(opened);
            simlin_project_unref(new);
        }

        let invalid = [0xffu8, 0xfe, 0];
        let mut err = ptr::null_mut();
        let new = simlin_project_new(invalid.as_ptr() as *const std::os::raw::c_char, &mut err);
        assert!(new.is_null(), "a name that is not UTF-8 makes no project");
        assert!(take_error(err).is_some(), "and says why");
    }
}
