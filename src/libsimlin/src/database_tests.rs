// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A project builds its salsa database the first time an entry point runs a
//! query, and never before.
//!
//! The entry points that run queries are the database's lock sites
//! (`grep -n 'lock_db' src`). Those that hold the datamodel lock pass the
//! contents they locked (`lock_db_with`): `simlin_sim_new`,
//! `simlin_project_get_errors`, `simlin_project_is_simulatable`,
//! `simlin_project_apply_patch` (every patch that validates),
//! `simlin_model_compile_to_wasm`, `simlin_analyze_discover_loops`,
//! `simlin_project_diagram_sync`, the renderings of a model with no view, and
//! the tool entry points (`simlin_tool_session_call`), which take the contents
//! they answer from under the datamodel lock.
//! Those that do not take the database alone (`lock_db`), which locks the
//! datamodel only to build it: `simlin_analyze_get_loops`,
//! `simlin_model_get_incoming_links`, `simlin_model_get_links` and
//! `simlin_model_get_latex_equation`. A row per entry point runs it first on a
//! project that has no database, on a thread of its own so that an entry point
//! waiting on a lock it holds fails the row rather than hanging it, and
//! requires the answer a project whose database was built at open gives.
//!
//! Four `lock_db` sites have no row. `simlin_analyze_get_loops_runtime` and
//! `simlin_analyze_get_links` take a simulation, whose `simlin_sim_new` built
//! the database; `simlin_analyze_links_from_wasm_results` and
//! `simlin_analyze_rel_loop_score_from_wasm_results` hold no datamodel lock,
//! so they build through the same `lock_db` the db-only rows exercise.
//!
//! Everything else reads or writes only the datamodel, so a project that is
//! only opened, copied, read, serialized, drawn or edited on its view never
//! builds one; and once a database exists, a query takes its lock alone, so
//! it answers while another thread holds the datamodel.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::ffi_error::SimlinError;
use crate::*;

/// A positive wait ("the other thread should get here"): `recv_timeout`
/// returns as soon as the message arrives, so the budget is spent only on a
/// genuine failure.
const POSITIVE_WAIT: Duration = Duration::from_secs(30);

/// A negative wait ("the other thread should not have answered yet"), which
/// slowness can only make pass.
const NEGATIVE_WAIT: Duration = Duration::from_millis(200);

/// A cloud filling a stock through a flow, which an aux drives, drawn when
/// `view` is set.
fn project_json(view: bool) -> Vec<u8> {
    let views = if view {
        json!([{"elements": [
            {"type": "stock", "uid": 1, "name": "population", "x": 100.0, "y": 100.0},
            {"type": "flow", "uid": 2, "name": "births", "x": 38.75, "y": 100.0, "points": [
                {"x": 0.0, "y": 100.0, "attachedToUid": 3},
                {"x": 77.5, "y": 100.0, "attachedToUid": 1}
            ]},
            {"type": "cloud", "uid": 3, "flowUid": 2, "x": 0.0, "y": 100.0},
            {"type": "aux", "uid": 4, "name": "rate", "x": 40.0, "y": 200.0},
            {"type": "link", "uid": 5, "fromUid": 4, "toUid": 2}
        ]}])
    } else {
        json!([])
    };
    serde_json::to_vec(&json!({
        "name": "database",
        "simSpecs": {"startTime": 0.0, "endTime": 10.0, "dt": "1"},
        "models": [{
            "name": "main",
            "stocks": [{"name": "population", "initialEquation": "10", "inflows": ["births"], "outflows": []}],
            "flows": [{"name": "births", "equation": "population * rate"}],
            "auxiliaries": [{"name": "rate", "equation": "0.1"}],
            "views": views
        }]
    }))
    .unwrap()
}

/// The error's code and message, freeing it; `None` for no error.
unsafe fn take_error(err: *mut SimlinError) -> Option<String> {
    if err.is_null() {
        return None;
    }
    let message = simlin_error_get_message(err);
    let message = if message.is_null() {
        String::new()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };
    let code = simlin_error_get_code(err);
    simlin_error_free(err);
    Some(format!("{code:?}: {message}"))
}

unsafe fn expect_no_error(err: *mut SimlinError, what: &str) {
    if let Some(error) = take_error(err) {
        panic!("{what} failed: {error}");
    }
}

unsafe fn open(view: bool) -> *mut SimlinProject {
    let bytes = project_json(view);
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

unsafe fn take_bytes(buf: *mut u8, len: usize) -> Vec<u8> {
    if buf.is_null() {
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(buf, len).to_vec();
    simlin_free(buf);
    bytes
}

unsafe fn take_string(s: *mut std::os::raw::c_char) -> String {
    if s.is_null() {
        return "NULL".to_string();
    }
    let owned = CStr::from_ptr(s).to_string_lossy().into_owned();
    simlin_free_string(s);
    owned
}

unsafe fn serialized_json(proj: *mut SimlinProject) -> Value {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_project_serialize_json(proj, 0, false, &mut buf, &mut len, &mut err);
    expect_no_error(err, "serializing the project");
    serde_json::from_slice(&take_bytes(buf, len)).expect("JSON")
}

/// The entry points that run queries, one row per database lock site that a
/// project with no database can reach.
#[derive(Clone, Copy, Debug)]
enum Query {
    SimNew,
    GetErrors,
    IsSimulatable,
    ValidatedPatch,
    CompileToWasm,
    DiscoverLoops,
    DiagramSync,
    RenderViewless,
    StructuralLoops,
    IncomingLinks,
    Links,
    LatexEquation,
    #[cfg(feature = "agent_tools")]
    ToolCall,
}

impl Query {
    const ALL: &[Query] = &[
        Query::SimNew,
        Query::GetErrors,
        Query::IsSimulatable,
        Query::ValidatedPatch,
        Query::CompileToWasm,
        Query::DiscoverLoops,
        Query::DiagramSync,
        Query::RenderViewless,
        Query::StructuralLoops,
        Query::IncomingLinks,
        Query::Links,
        Query::LatexEquation,
        #[cfg(feature = "agent_tools")]
        Query::ToolCall,
    ];

    /// Whether the row runs on the project with no stock-and-flow view.
    fn viewless(self) -> bool {
        matches!(self, Query::RenderViewless)
    }

    /// What the entry point answers, as text a row compares.
    unsafe fn answer(self, proj: *mut SimlinProject) -> String {
        let model = main_model(proj);
        let answer = self.answer_through(proj, model);
        simlin_model_unref(model);
        answer
    }

    /// What the entry point answers through a model handle already in hand,
    /// so the answer takes no lock but the entry point's own.
    unsafe fn answer_through(self, proj: *mut SimlinProject, model: *mut SimlinModel) -> String {
        let main = CString::new("main").unwrap();
        let births = CString::new("births").unwrap();
        let mut err = ptr::null_mut();
        let answer = match self {
            Query::SimNew => {
                let sim = simlin_sim_new(model, false, &mut err);
                expect_no_error(err, "creating a simulation");
                simlin_sim_run_to_end(sim, &mut err);
                expect_no_error(err, "running the simulation");
                let population = CString::new("population").unwrap();
                let mut values = vec![0.0; 11];
                let mut written = 0;
                simlin_sim_get_series(
                    sim,
                    population.as_ptr(),
                    values.as_mut_ptr(),
                    values.len(),
                    &mut written,
                    &mut err,
                );
                simlin_sim_unref(sim);
                format!("{:?}", &values[..written])
            }
            Query::GetErrors => {
                let errors = simlin_project_get_errors(proj, &mut err);
                format!("{:?}", take_error(errors))
            }
            Query::IsSimulatable => {
                format!(
                    "{}",
                    simlin_project_is_simulatable(proj, main.as_ptr(), &mut err)
                )
            }
            Query::ValidatedPatch => {
                let patch = serde_json::to_vec(&json!({"models": [{"name": "main", "ops": [
                    {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "rate +"}}}
                ]}]}))
                .unwrap();
                let (mut collected, mut rejected) = (ptr::null_mut(), ptr::null_mut());
                simlin_project_apply_patch(
                    proj,
                    patch.as_ptr(),
                    patch.len(),
                    true,
                    false,
                    &mut collected,
                    &mut rejected,
                );
                format!("{:?} {:?}", take_error(collected), take_error(rejected))
            }
            Query::CompileToWasm => {
                let (mut wasm, mut wasm_len) = (ptr::null_mut(), 0);
                let (mut layout, mut layout_len) = (ptr::null_mut(), 0);
                simlin_model_compile_to_wasm(
                    model,
                    false,
                    false,
                    &mut wasm,
                    &mut wasm_len,
                    &mut layout,
                    &mut layout_len,
                    &mut err,
                );
                // The layout's name map is compared as a map: its entries
                // come out in the order of the compiled program's offset map,
                // a `HashMap`, which differs between two databases.
                let layout = simlin_engine::wasmgen::WasmLayout::deserialize(&take_bytes(
                    layout, layout_len,
                ))
                .map(|layout| {
                    let mut names = layout.var_offsets;
                    names.sort();
                    (
                        layout.n_slots,
                        layout.n_chunks,
                        layout.results_offset,
                        names,
                    )
                });
                format!("{:?} {layout:?}", take_bytes(wasm, wasm_len))
            }
            Query::DiscoverLoops => {
                let result = simlin_analyze_discover_loops(model, 0, &mut err);
                let count = if result.is_null() {
                    None
                } else {
                    let count = (*result).loop_count;
                    simlin_free_discovery_result(result);
                    Some(count)
                };
                format!("{count:?}")
            }
            Query::DiagramSync => {
                simlin_project_diagram_sync(proj, main.as_ptr(), ptr::null(), &mut err);
                serialized_json(proj).to_string()
            }
            Query::RenderViewless => {
                let (mut buf, mut len) = (ptr::null_mut(), 0);
                simlin_project_render_svg(proj, main.as_ptr(), &mut buf, &mut len, &mut err);
                String::from_utf8_lossy(&take_bytes(buf, len)).into_owned()
            }
            Query::StructuralLoops => {
                let loops = simlin_analyze_get_loops(model, &mut err);
                let ids = if loops.is_null() {
                    Vec::new()
                } else {
                    let list = std::slice::from_raw_parts((*loops).loops, (*loops).count);
                    let ids = list
                        .iter()
                        .map(|l| CStr::from_ptr(l.id).to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    simlin_free_loops(loops);
                    ids
                };
                format!("{ids:?}")
            }
            Query::IncomingLinks => {
                let mut names = vec![ptr::null_mut(); 8];
                let mut written = 0;
                simlin_model_get_incoming_links(
                    model,
                    births.as_ptr(),
                    names.as_mut_ptr(),
                    names.len(),
                    &mut written,
                    &mut err,
                );
                let names: Vec<String> = names
                    .into_iter()
                    .take(written)
                    .map(|n| take_string(n))
                    .collect();
                format!("{names:?}")
            }
            Query::Links => {
                let links = simlin_model_get_links(model, &mut err);
                let pairs = if links.is_null() {
                    Vec::new()
                } else {
                    let list = std::slice::from_raw_parts((*links).links, (*links).count);
                    let pairs = list
                        .iter()
                        .map(|l| {
                            (
                                CStr::from_ptr(l.from).to_string_lossy().into_owned(),
                                CStr::from_ptr(l.to).to_string_lossy().into_owned(),
                            )
                        })
                        .collect::<Vec<_>>();
                    simlin_free_links(links);
                    pairs
                };
                let mut pairs = pairs;
                pairs.sort();
                format!("{pairs:?}")
            }
            Query::LatexEquation => take_string(simlin_model_get_latex_equation(
                model,
                births.as_ptr(),
                &mut err,
            )),
            #[cfg(feature = "agent_tools")]
            Query::ToolCall => {
                let session = crate::tools::simlin_tool_session_new(model, &mut err);
                expect_no_error(err, "making a tool session");
                let tool = CString::new("read_model").unwrap();
                let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0, false);
                crate::tools::simlin_tool_session_call(
                    session,
                    tool.as_ptr(),
                    ptr::null(),
                    0,
                    &mut buf,
                    &mut len,
                    &mut is_error,
                    &mut err,
                );
                crate::tools::simlin_tool_session_unref(session);
                format!(
                    "{is_error} {}",
                    String::from_utf8_lossy(&take_bytes(buf, len))
                )
            }
        };
        let error = take_error(err);
        format!("{answer} / {error:?}")
    }
}

/// `query`'s answer on `proj`, from a thread of its own; `None` when it has
/// not answered within `wait`.
fn answer_on_a_thread(query: Query, proj: *mut SimlinProject, wait: Duration) -> Option<String> {
    let proj_addr = proj as usize;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let answer = unsafe { query.answer(proj_addr as *mut SimlinProject) };
        let _ = tx.send(answer);
    });
    rx.recv_timeout(wait).ok()
}

#[test]
fn every_entry_point_that_queries_builds_the_database_and_answers_as_one_built_at_open_does() {
    let mut failures = Vec::new();
    for &query in Query::ALL {
        unsafe {
            let built = open(!query.viewless());
            drop((*built).lock_db());
            let expected = query.answer(built);

            let fresh = open(!query.viewless());
            if (*fresh).has_db() {
                failures.push(format!("{query:?}: opening built a database"));
            }
            match answer_on_a_thread(query, fresh, POSITIVE_WAIT) {
                None => failures.push(format!(
                    "{query:?} never answered on a project with no database"
                )),
                Some(answer) => {
                    if answer != expected {
                        failures.push(format!(
                            "{query:?} answered {answer} on a project with no database, {expected} on one built at open"
                        ));
                    }
                    if !(*fresh).has_db() {
                        failures.push(format!("{query:?} answered without a database"));
                    }
                }
            }
            // A row that never answered leaves its thread waiting on the
            // project, which is then leaked rather than freed under it.
            if !failures.iter().any(|f| f.contains("never answered")) {
                simlin_project_unref(fresh);
            }
            simlin_project_unref(built);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Entry points that read or write only the datamodel.
#[derive(Clone, Copy, Debug)]
enum DatamodelOnly {
    Copy,
    SerializeJson,
    SerializeXmile,
    SerializeMdl,
    SerializeProtobuf,
    RenderScene,
    RenderSvg,
    VariableNames,
    VariableJson,
    HitTest,
    ViewOnlyPatch,
    AddModel,
}

impl DatamodelOnly {
    const ALL: [DatamodelOnly; 12] = [
        DatamodelOnly::Copy,
        DatamodelOnly::SerializeJson,
        DatamodelOnly::SerializeXmile,
        DatamodelOnly::SerializeMdl,
        DatamodelOnly::SerializeProtobuf,
        DatamodelOnly::RenderScene,
        DatamodelOnly::RenderSvg,
        DatamodelOnly::VariableNames,
        DatamodelOnly::VariableJson,
        DatamodelOnly::HitTest,
        DatamodelOnly::ViewOnlyPatch,
        DatamodelOnly::AddModel,
    ];

    /// Runs the entry point on `proj`; a copy is returned for the row to check too.
    unsafe fn run(self, proj: *mut SimlinProject) -> Option<*mut SimlinProject> {
        let model = main_model(proj);
        let main = CString::new("main").unwrap();
        let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
        let mut copy = None;
        match self {
            DatamodelOnly::Copy => {
                let new = simlin_project_new(ptr::null(), &mut err);
                expect_no_error(err, "a new project");
                simlin_project_replace_contents(new, proj, &mut err);
                copy = Some(new);
            }
            DatamodelOnly::SerializeJson => {
                simlin_project_serialize_json(proj, 0, false, &mut buf, &mut len, &mut err)
            }
            DatamodelOnly::SerializeXmile => {
                simlin_project_serialize_xmile(proj, &mut buf, &mut len, &mut err)
            }
            DatamodelOnly::SerializeMdl => {
                simlin_project_serialize_mdl(proj, &mut buf, &mut len, ptr::null_mut(), &mut err)
            }
            DatamodelOnly::SerializeProtobuf => {
                simlin_project_serialize_protobuf(proj, &mut buf, &mut len, &mut err)
            }
            DatamodelOnly::RenderScene => {
                simlin_project_render_scene(proj, main.as_ptr(), &mut buf, &mut len, &mut err)
            }
            DatamodelOnly::RenderSvg => {
                simlin_project_render_svg(proj, main.as_ptr(), &mut buf, &mut len, &mut err)
            }
            DatamodelOnly::VariableNames => {
                let mut count = 0;
                simlin_model_get_var_count(model, 0, ptr::null(), &mut count, &mut err);
            }
            DatamodelOnly::VariableJson => {
                let births = CString::new("births").unwrap();
                simlin_model_get_var_json(model, births.as_ptr(), &mut buf, &mut len, &mut err);
            }
            DatamodelOnly::HitTest => {
                let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
                simlin_model_hit_test(
                    model, 100.0, 100.0, 6.0, &mut hit, &mut uid, &mut part, &mut err,
                );
            }
            DatamodelOnly::ViewOnlyPatch => {
                let patch = serde_json::to_vec(&json!({"models": [{"name": "main", "ops": [
                    {"type": "editView", "payload": {"index": 0, "upsert": [
                        {"type": "stock", "uid": 1, "name": "population", "x": 300.0, "y": 300.0}
                    ], "remove": []}}
                ]}]}))
                .unwrap();
                let mut collected = ptr::null_mut();
                simlin_project_apply_patch(
                    proj,
                    patch.as_ptr(),
                    patch.len(),
                    false,
                    false,
                    &mut collected,
                    &mut err,
                );
                expect_no_error(collected, "collecting the patch's errors");
            }
            DatamodelOnly::AddModel => {
                let name = CString::new("another").unwrap();
                simlin_project_add_model(proj, name.as_ptr(), &mut err);
            }
        }
        expect_no_error(err, &format!("{self:?}"));
        take_bytes(buf, len);
        simlin_model_unref(model);
        copy
    }
}

#[test]
fn a_project_that_is_only_read_written_drawn_or_copied_builds_no_database() {
    let mut failures = Vec::new();
    for entry in DatamodelOnly::ALL {
        unsafe {
            let proj = open(true);
            let copy = entry.run(proj);
            if (*proj).has_db() {
                failures.push(format!("{entry:?} built a database"));
            }
            if let Some(copy) = copy {
                if (*copy).has_db() {
                    failures.push(format!("{entry:?} built the copy a database"));
                }
                simlin_project_unref(copy);
            }
            simlin_project_unref(proj);
        }
    }
    let mut err = ptr::null_mut();
    unsafe {
        let new = simlin_project_new(ptr::null(), &mut err);
        expect_no_error(err, "a new project");
        if (*new).has_db() {
            failures.push("a new project built a database".to_string());
        }
        simlin_project_unref(new);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_copy_of_a_compiled_project_builds_a_database_of_its_own_only_when_queried() {
    unsafe {
        let original = open(true);
        let expected = Query::SimNew.answer(original);
        let copy = {
            let mut err = ptr::null_mut();
            let new = simlin_project_new(ptr::null(), &mut err);
            expect_no_error(err, "a new project");
            simlin_project_replace_contents(new, original, &mut err);
            expect_no_error(err, "copying the project");
            new
        };
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
            Query::SimNew.answer(original),
            expected,
            "the edit changes what the original simulates"
        );
        assert_eq!(
            Query::SimNew.answer(copy),
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
    unsafe {
        let proj = open(true);
        drop((*proj).lock_db());
        let model = main_model(proj);
        let expected = Query::LatexEquation.answer_through(proj, model);
        let held = (*proj).datamodel.lock().unwrap();
        let (proj_addr, model_addr) = (proj as usize, model as usize);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let answer = Query::LatexEquation.answer_through(
                proj_addr as *mut SimlinProject,
                model_addr as *mut SimlinModel,
            );
            let _ = tx.send(answer);
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
    unsafe {
        let proj = open(true);
        let unedited = open(true);
        let before = Query::LatexEquation.answer(unedited);
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
            let answer = Query::LatexEquation.answer(proj_addr as *mut SimlinProject);
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
                **(*new).datamodel.lock().unwrap() == **(*opened).datamodel.lock().unwrap(),
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
