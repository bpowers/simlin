// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The agent tool FFI, from a model handle to a tool's JSON.
//!
//! What each tool answers is the engine's decision, pinned by the tests in
//! `simlin_engine::tools`. These tests pin the boundary: the catalog a host
//! reads is the engine's, a call carries JSON in and out at the project's
//! current revision, a refusal is output the agent reads rather than an error,
//! a host's misuse is an error with a code, a session keeps its model alive,
//! calls interleave with edits from other threads without deadlock, and a loop
//! analysis leaves the host's other analyses of the project as they were.

use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use simlin::*;
use simlin_engine::test_common::TestProject;

use crate::common::{expect_error_code, expect_no_error, open_project_from_datamodel};

fn project() -> *mut SimlinProject {
    let project = TestProject::new("tools")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "10", &["births"], &[], Some("person"))
        .flow("births", "population * rate", Some("person/month"))
        .aux("rate", "0.1", Some("1/month"))
        .build_datamodel();
    open_project_from_datamodel(&project)
}

unsafe fn main_model(proj: *mut SimlinProject) -> *mut SimlinModel {
    let name = CString::new("main").unwrap();
    let mut err = ptr::null_mut();
    let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
    expect_no_error(err, "getting main");
    model
}

unsafe fn new_session(model: *mut SimlinModel) -> *mut SimlinToolSession {
    let mut err = ptr::null_mut();
    let session = simlin_tool_session_new(model, &mut err);
    expect_no_error(err, "making a tool session");
    assert!(!session.is_null());
    session
}

/// How long a call may take before the test says it waits on a lock it can
/// never take; `recv_timeout` returns as soon as the call answers.
const CALL_WAIT: Duration = Duration::from_secs(30);

/// Call `tool` with `input` and return the output and whether it is a refusal.
/// The call runs on a thread of its own, so one that waits on a lock it holds
/// fails the test instead of hanging it.
unsafe fn call(session: *mut SimlinToolSession, tool: &str, input: &str) -> (Value, bool) {
    let (session_addr, tool, input) = (session as usize, tool.to_string(), input.to_string());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let name = CString::new(tool.as_str()).unwrap();
        let (mut buf, mut len, mut is_error, mut err) =
            (ptr::null_mut(), 0, false, ptr::null_mut());
        simlin_tool_session_call(
            session_addr as *mut SimlinToolSession,
            name.as_ptr(),
            input.as_ptr(),
            input.len(),
            &mut buf,
            &mut len,
            &mut is_error,
            &mut err,
        );
        let _ = tx.send((buf as usize, len, is_error, err as usize, tool));
    });
    let (buf, len, is_error, err, tool) = rx
        .recv_timeout(CALL_WAIT)
        .expect("the call answers rather than waiting on a lock it can never take");
    expect_no_error(err as *mut SimlinError, &tool);
    let buf = buf as *mut u8;
    let output = serde_json::from_slice(std::slice::from_raw_parts(buf, len)).expect("JSON");
    simlin_free(buf);
    (output, is_error)
}

unsafe fn changes(session: *mut SimlinToolSession) -> Value {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_tool_session_get_changes(session, &mut buf, &mut len, &mut err);
    expect_no_error(err, "reading the changes");
    let changes = serde_json::from_slice(std::slice::from_raw_parts(buf, len)).expect("JSON");
    simlin_free(buf);
    changes
}

unsafe fn apply(proj: *mut SimlinProject, patch: &Value) {
    let bytes = serde_json::to_vec(patch).unwrap();
    let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
    simlin_project_apply_patch(
        proj,
        bytes.as_ptr(),
        bytes.len(),
        false,
        false,
        &mut collected,
        &mut err,
    );
    if !collected.is_null() {
        simlin_error_free(collected);
    }
    expect_no_error(err, "the patch");
}

fn set_rate(equation: &str) -> Value {
    json!({"models": [{"name": "main", "ops": [
        {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": equation, "units": "1/month"}}}
    ]}]})
}

#[test]
fn the_catalog_a_host_reads_is_the_engines() {
    unsafe {
        let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
        simlin_tools_describe(&mut buf, &mut len, &mut err);
        expect_no_error(err, "describing the tools");
        let bytes = std::slice::from_raw_parts(buf, len);
        assert_eq!(bytes, simlin_engine::tools::catalog_json().as_bytes());
        simlin_free(buf);

        let mut err = ptr::null_mut();
        simlin_tools_describe(ptr::null_mut(), &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL buffer pointer");
    }
}

#[test]
fn a_call_answers_with_json_at_the_projects_revision_and_reports_the_changes_since_its_read() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        assert_eq!(changes(session), Value::Null, "nothing was read yet");

        let (outline, is_error) = call(session, "read_model", "{}");
        assert!(!is_error, "{outline}");
        assert_eq!(outline["revision"], 0);
        assert_eq!(outline["stocks"][0]["name"], "population");

        apply(proj, &set_rate("0.2"));
        let mut revision = 0;
        let mut err = ptr::null_mut();
        simlin_project_get_revision(proj, &mut revision, &mut err);
        expect_no_error(err, "reading the revision");
        assert!(revision > 0);
        assert_eq!(
            changes(session)["changed"],
            json!([{"name": "rate", "fields": ["equation"]}])
        );

        let (record, is_error) = call(session, "read_variables", r#"{"names": ["rate"]}"#);
        assert!(!is_error, "{record}");
        assert_eq!(record["revision"], revision);
        assert_eq!(record["variables"][0]["equation"], "0.2");

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_refusal_is_output_for_the_agent_and_not_an_error() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        let (refusal, is_error) = call(session, "read_variables", r#"{"nmes": ["rate"]}"#);
        assert!(is_error);
        assert!(
            refusal["error"].as_str().unwrap().contains("nmes"),
            "{refusal}"
        );
        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn an_empty_input_is_an_empty_object() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        let name = CString::new("read_model").unwrap();
        let (mut buf, mut len, mut is_error, mut err) = (ptr::null_mut(), 0, true, ptr::null_mut());
        simlin_tool_session_call(
            session,
            name.as_ptr(),
            ptr::null(),
            0,
            &mut buf,
            &mut len,
            &mut is_error,
            &mut err,
        );
        expect_no_error(err, "a call with no input");
        assert!(!is_error);
        simlin_free(buf);
        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_hosts_misuse_is_an_error_with_a_code_and_writes_nothing() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        let read_model = CString::new("read_model").unwrap();
        let unknown = CString::new("read_everything").unwrap();
        let input = "{}";
        let bad_utf8 = [0xffu8, 0xfe];

        type Row<'a> = (
            &'a str,
            *mut SimlinToolSession,
            *const c_char,
            *const u8,
            usize,
            SimlinErrorCode,
        );
        let rows: [Row; 5] = [
            (
                "an unknown tool",
                session,
                unknown.as_ptr(),
                input.as_ptr(),
                input.len(),
                SimlinErrorCode::DoesNotExist,
            ),
            (
                "a NULL session",
                ptr::null_mut(),
                read_model.as_ptr(),
                input.as_ptr(),
                input.len(),
                SimlinErrorCode::Generic,
            ),
            (
                "a NULL name",
                session,
                ptr::null(),
                input.as_ptr(),
                input.len(),
                SimlinErrorCode::Generic,
            ),
            (
                "a NULL input with a length",
                session,
                read_model.as_ptr(),
                ptr::null(),
                2,
                SimlinErrorCode::Generic,
            ),
            (
                "input that is not UTF-8",
                session,
                read_model.as_ptr(),
                bad_utf8.as_ptr(),
                2,
                SimlinErrorCode::Generic,
            ),
        ];
        for (what, session, name, input, input_len, code) in rows {
            let (mut buf, mut len, mut is_error, mut err) =
                (ptr::null_mut(), 0, false, ptr::null_mut());
            simlin_tool_session_call(
                session,
                name,
                input,
                input_len,
                &mut buf,
                &mut len,
                &mut is_error,
                &mut err,
            );
            expect_error_code(err, code, what);
            assert!(buf.is_null(), "{what} wrote no output");
        }

        let mut err = ptr::null_mut();
        let (mut len, mut is_error) = (0, false);
        simlin_tool_session_call(
            session,
            read_model.as_ptr(),
            input.as_ptr(),
            input.len(),
            ptr::null_mut(),
            &mut len,
            &mut is_error,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL output pointer");

        let mut err = ptr::null_mut();
        assert!(simlin_tool_session_new(ptr::null_mut(), &mut err).is_null());
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL model");

        let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
        simlin_tool_session_get_changes(ptr::null_mut(), &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "changes of a NULL session");

        // A NULL handle is a no-op to reference and release.
        simlin_tool_session_ref(ptr::null_mut());
        simlin_tool_session_unref(ptr::null_mut());

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_session_keeps_its_model_and_project_alive() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        simlin_tool_session_ref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
        let (outline, is_error) = call(session, "read_model", "{}");
        assert!(!is_error && outline["model"] == "main", "{outline}");
        simlin_tool_session_unref(session);
        let (outline, is_error) = call(session, "find_variables", r#"{"phrase": "pop"}"#);
        assert!(!is_error && outline["matches"][0]["name"] == "population");
        simlin_tool_session_unref(session);
    }
}

/// A tool call locks the session, the datamodel and the db in that order, and
/// a patch the datamodel and the db, so calls and edits from two threads
/// interleave without deadlock: every call answers at a revision some edit
/// left, or stops for the edit that waits for it.
#[test]
fn calls_and_edits_from_two_threads_interleave_without_deadlock() {
    const ROUNDS: usize = 25;
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        let (proj_addr, session_addr) = (proj as usize, session as usize);
        let (done_tx, done_rx) = mpsc::channel();

        let editor_done = done_tx.clone();
        let editor = thread::spawn(move || {
            for i in 0..ROUNDS {
                apply(
                    proj_addr as *mut SimlinProject,
                    &set_rate(&format!("0.{}", i + 1)),
                );
            }
            let _ = editor_done.send("editor");
        });
        let caller = thread::spawn(move || {
            let mut last = 0;
            for _ in 0..ROUNDS {
                let (outline, is_error) =
                    call(session_addr as *mut SimlinToolSession, "read_model", "{}");
                // A call an edit waits for stops for it, and says so.
                if is_error {
                    assert_eq!(outline["interrupted"], true, "{outline}");
                    continue;
                }
                let revision = outline["revision"].as_u64().unwrap();
                assert!(revision >= last, "the revision never goes back");
                last = revision;
            }
            let _ = done_tx.send("caller");
        });
        for _ in 0..2 {
            done_rx
                .recv_timeout(Duration::from_secs(60))
                .expect("both threads finish");
        }
        editor.join().unwrap();
        caller.join().unwrap();

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A run's series, through the results handle a host charts from.
unsafe fn run_series(session: *mut SimlinToolSession, run: &str, variable: &str) -> Vec<f64> {
    let name = CString::new(run).unwrap();
    let mut err = ptr::null_mut();
    let results = simlin_tool_session_get_run(
        session,
        name.as_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
        &mut err,
    );
    expect_no_error(err, "getting the run");
    assert!(!results.is_null());
    let mut steps = 0;
    let mut err = ptr::null_mut();
    simlin_results_get_stepcount(results, &mut steps, &mut err);
    expect_no_error(err, "counting the run's steps");
    let variable = CString::new(variable).unwrap();
    let mut series = vec![0.0; steps];
    let (mut written, mut err) = (0, ptr::null_mut());
    simlin_results_get_series(
        results,
        variable.as_ptr(),
        series.as_mut_ptr(),
        series.len(),
        &mut written,
        &mut err,
    );
    expect_no_error(err, "reading the series");
    assert_eq!(written, steps);
    simlin_results_unref(results);
    series
}

/// How many loops the host's own structural loop surface reports.
unsafe fn structural_loop_count(model: *mut SimlinModel) -> usize {
    let mut err = ptr::null_mut();
    let loops = simlin_analyze_get_loops(model, &mut err);
    expect_no_error(err, "the structural loops");
    let count = (*loops).count;
    simlin_free_loops(loops);
    count
}

#[test]
fn a_loop_analysis_follows_the_project_and_leaves_its_other_analyses_as_they_were() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        assert_eq!(structural_loop_count(model), 1);

        let (loops, is_error) = call(session, "analyze_loops", "{}");
        assert!(!is_error, "{loops}");
        assert_eq!(loops["basis"], "run");
        let growth = &loops["partitions"][0]["loops"][0];
        assert_eq!(growth["id"], "L1");
        assert_eq!(growth["polarity"], "reinforcing");
        // The analysis runs in discovery mode and sets the project back, so
        // the structural surface still enumerates the model's loops.
        assert_eq!(structural_loop_count(model), 1);

        // A rate of zero holds the population still: the same loop, from
        // structure, under its id.
        apply(proj, &set_rate("0"));
        let (still, is_error) = call(session, "analyze_loops", "{}");
        assert!(!is_error, "{still}");
        assert!(still["revision"].as_u64().unwrap() > 0);
        assert_eq!(still["basis"], "structure");
        assert_eq!(still["partitions"][0]["loops"][0]["id"], "L1");
        assert_eq!(structural_loop_count(model), 1);

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_host_charts_a_runs_series_from_its_results_handle() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        let current = run_series(session, "current", "population");
        assert_eq!(current.len(), 11);
        assert_eq!(current[0], 10.0);

        let (output, is_error) = call(
            session,
            "run_experiment",
            r#"{"name": "faster", "set": [{"variable": "rate", "multiply": 2}]}"#,
        );
        assert!(!is_error, "{output}");
        let faster = run_series(session, "faster", "population");
        assert_eq!(faster[0], 10.0);
        assert!(faster[10] > current[10], "{faster:?} {current:?}");

        let unknown = CString::new("nowhere").unwrap();
        let mut err = ptr::null_mut();
        let (none, no) = (ptr::null_mut(), ptr::null_mut());
        assert!(
            simlin_tool_session_get_run(session, unknown.as_ptr(), none, no, &mut err).is_null()
        );
        expect_error_code(
            err,
            SimlinErrorCode::DoesNotExist,
            "a run the session lacks",
        );
        let mut err = ptr::null_mut();
        assert!(simlin_tool_session_get_run(session, ptr::null(), none, no, &mut err).is_null());
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL run name");
        let mut err = ptr::null_mut();
        assert!(
            simlin_tool_session_get_run(ptr::null_mut(), unknown.as_ptr(), none, no, &mut err)
                .is_null()
        );
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL session");

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The session's run list, as a host's run picker reads it.
unsafe fn list_runs(session: *mut SimlinToolSession) -> serde_json::Value {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_tool_session_list_runs(session, &mut buf, &mut len, &mut err);
    expect_no_error(err, "listing the runs");
    let json = std::slice::from_raw_parts(buf, len).to_vec();
    simlin_free(buf);
    serde_json::from_slice(&json).unwrap()
}

/// A host lists the session's runs and learns, for each and for the series it
/// charts, the revision it was made at and whether the model has changed
/// since: not when only the diagram moved, and yes after an equation edit.
#[test]
fn a_host_lists_runs_and_learns_when_one_is_stale() {
    unsafe {
        let proj = project();
        let model = main_model(proj);
        let session = new_session(model);
        assert_eq!(list_runs(session), serde_json::json!([]));
        let (output, is_error) = call(
            session,
            "run_experiment",
            r#"{"name": "faster", "set": [{"variable": "rate", "multiply": 2}]}"#,
        );
        assert!(!is_error, "{output}");
        assert_eq!(
            list_runs(session),
            serde_json::json!([{
                "name": "faster", "revision": 0, "stale": false, "gone": false,
                "from": "current", "changes": [{"variable": "rate", "value": 0.2}], "specs": {}
            }])
        );

        let fetch = |session| {
            let name = CString::new("faster").unwrap();
            let (mut revision, mut stale, mut err) = (u64::MAX, true, ptr::null_mut());
            let results = simlin_tool_session_get_run(
                session,
                name.as_ptr(),
                &mut revision,
                &mut stale,
                &mut err,
            );
            expect_no_error(err, "getting the run");
            simlin_results_unref(results);
            (revision, stale)
        };
        assert_eq!(fetch(session), (0, false));

        // A diagram edit advances the revision and changes nothing the run
        // simulated.
        apply(
            proj,
            &json!({"models": [{"name": "main", "ops": [
                {"type": "upsertView", "payload": {"index": 0, "view": {"kind": "stock_flow", "elements": []}}}
            ]}]}),
        );
        let mut revision = 0;
        let mut err = ptr::null_mut();
        simlin_project_get_revision(proj, &mut revision, &mut err);
        expect_no_error(err, "reading the revision");
        assert!(revision > 0, "the diagram edit landed");
        assert_eq!(
            fetch(session),
            (0, false),
            "a diagram edit leaves the run fresh"
        );
        apply(proj, &set_rate("0.3"));
        assert_eq!(fetch(session), (0, true), "an equation edit makes it stale");
        assert_eq!(list_runs(session)[0]["stale"], true);

        let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
        simlin_tool_session_list_runs(ptr::null_mut(), &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL session");

        simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
