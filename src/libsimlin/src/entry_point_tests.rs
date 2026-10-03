// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every entry point that reaches a project, in one table, and the rules a
//! row's place in the table commits it to.
//!
//! A row ([`EntryPoint`]) names an exported function, says which of the
//! project's locks it takes and how ([`Footprint`]), says what it changes
//! ([`Writes`]), and runs it. The rules are stated once over the rows, so an
//! entry point is held to them by having a row, and
//! `every_exported_function_has_a_row_or_reaches_no_project` reads every
//! function `simlin.h` declares (cbindgen's output, which the hook and CI hold
//! fresh) and requires of each a row or a reason in `REACHES_NO_PROJECT`; a
//! function given there takes no project, model or tool session, which the
//! scan checks, while that it reaches no project through what it IS handed
//! (a simulation, a gesture, a results table) is the reason's claim.
//!
//! What a row declares is observed, not trusted: the footprint from the lock
//! events the call makes on its own thread (`lock_order::trace`), which also
//! say whether it works on the database while it holds the contents; what it
//! writes from the revision, the hit index and a copy that shares the
//! datamodel. From those two columns follow whether it builds a database on
//! a project that has none, whether it counts itself while it waits for the
//! database, and whether the database it leaves answers as one built from
//! the contents it leaves.
//!
//! `database_tests.rs` and `contents_tests.rs` hold the rules that are about
//! one entry point rather than a column of the table.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use simlin_engine::editing::{self, Point};

use crate::ffi_error::SimlinError;
use crate::lock_order::{self, Event, Rank};
use crate::*;

/// A positive wait ("the other thread should get here"): `recv_timeout`
/// returns as soon as the message arrives, so the budget is spent only on a
/// genuine failure.
pub(crate) const POSITIVE_WAIT: Duration = Duration::from_secs(30);

/// A negative wait ("the other thread should not have answered yet"), which
/// slowness can only make pass.
pub(crate) const NEGATIVE_WAIT: Duration = Duration::from_millis(200);

pub(crate) const STOCK: i32 = 1;
pub(crate) const TOLERANCE: f64 = 10.0;

// ── the fixture ────────────────────────────────────────────────────────

/// A cloud filling a stock through a flow, which an aux drives: drawn, with
/// the stock at `(stock_x, stock_y)`, when `view` is set.
pub(crate) fn project_json(view: bool, stock_x: f64, stock_y: f64, rate: &str) -> Vec<u8> {
    let views = if view {
        json!([{"elements": [
            {"type": "stock", "uid": STOCK, "name": "population", "x": stock_x, "y": stock_y},
            {"type": "flow", "uid": 2, "name": "births", "x": 38.75, "y": 100.0, "points": [
                {"x": 0.0, "y": 100.0, "attachedToUid": 3},
                {"x": 77.5, "y": 100.0, "attachedToUid": STOCK}
            ]},
            {"type": "cloud", "uid": 3, "flowUid": 2, "x": 0.0, "y": 100.0},
            {"type": "aux", "uid": 4, "name": "rate", "x": 40.0, "y": 200.0},
            {"type": "link", "uid": 5, "fromUid": 4, "toUid": 2}
        ]}])
    } else {
        json!([])
    };
    serde_json::to_vec(&json!({
        "name": "entry points",
        "simSpecs": {"startTime": 0.0, "endTime": 10.0, "dt": "1"},
        "models": [{
            "name": "main",
            "stocks": [{"name": "population", "initialEquation": "10", "inflows": ["births"], "outflows": []}],
            "flows": [{"name": "births", "equation": "population * rate"}],
            "auxiliaries": [{"name": "rate", "equation": rate}],
            "views": views
        }]
    }))
    .unwrap()
}

/// The error's code and message, freeing it; `None` for no error.
pub(crate) unsafe fn take_error(err: *mut SimlinError) -> Option<String> {
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

pub(crate) unsafe fn expect_no_error(err: *mut SimlinError, what: &str) {
    if let Some(error) = take_error(err) {
        panic!("{what} failed: {error}");
    }
}

unsafe fn open_json(bytes: &[u8]) -> *mut SimlinProject {
    let mut err = ptr::null_mut();
    let proj = simlin_project_open_json(bytes.as_ptr(), bytes.len(), 0, &mut err);
    expect_no_error(err, "opening the project");
    proj
}

/// The fixture, drawn when `view` is set.
pub(crate) unsafe fn open(view: bool) -> *mut SimlinProject {
    open_json(&project_json(view, 100.0, 100.0, "0.1"))
}

pub(crate) unsafe fn main_model(proj: *mut SimlinProject) -> *mut SimlinModel {
    let name = CString::new("main").unwrap();
    let mut err = ptr::null_mut();
    let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
    expect_no_error(err, "getting main");
    model
}

/// A copy of `proj`, made as a host copies a project: its contents replaced
/// with `proj`'s in a new project.
pub(crate) unsafe fn copy_of(proj: *mut SimlinProject) -> *mut SimlinProject {
    let mut err = ptr::null_mut();
    let copy = simlin_project_new(ptr::null(), &mut err);
    expect_no_error(err, "a new project");
    simlin_project_replace_contents(copy, proj, &mut err);
    expect_no_error(err, "copying the project");
    copy
}

pub(crate) unsafe fn shares_datamodel(a: *mut SimlinProject, b: *mut SimlinProject) -> bool {
    let a = (*a).datamodel.lock().unwrap().shared();
    let b = (*b).datamodel.lock().unwrap().shared();
    Arc::ptr_eq(&a, &b)
}

pub(crate) unsafe fn datamodel_of(
    proj: *mut SimlinProject,
) -> Arc<simlin_engine::datamodel::Project> {
    (*proj).datamodel.lock().unwrap().shared()
}

pub(crate) unsafe fn has_index(proj: *mut SimlinProject) -> bool {
    (*proj).datamodel.lock().unwrap().has_hit_index("main")
}

/// The project's revision, through the FFI.
pub(crate) unsafe fn revision(proj: *mut SimlinProject) -> u64 {
    let (mut revision, mut err) = (0, ptr::null_mut());
    simlin_project_get_revision(proj, &mut revision, &mut err);
    expect_no_error(err, "reading the revision");
    revision
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

pub(crate) unsafe fn apply_patch(
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

pub(crate) fn edit_view(upsert: Value) -> Value {
    json!({"models": [{"name": "main", "ops": [{"type": "editView", "payload": {"index": 0, "upsert": [upsert], "remove": []}}]}]})
}

/// Moves the stock, which implies no model operation.
pub(crate) fn move_stock() -> Value {
    edit_view(json!({"type": "stock", "uid": STOCK, "name": "population", "x": 400.0, "y": 400.0}))
}

/// The points a row asks the hit test about: a grid over the diagram and
/// around it.
fn points() -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for i in 0..30 {
        for j in 0..30 {
            out.push((-150.0 + 25.0 * i as f64, -150.0 + 25.0 * j as f64));
        }
    }
    out
}

pub(crate) type Hit = Option<(i32, SimlinHitPart)>;

/// What the hit test answers at `(x, y)`, through the FFI.
pub(crate) unsafe fn hit_at(model: *mut SimlinModel, (x, y): (f64, f64)) -> Hit {
    let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
    let mut err = ptr::null_mut();
    simlin_model_hit_test(
        model, x, y, TOLERANCE, &mut hit, &mut uid, &mut part, &mut err,
    );
    expect_no_error(err, "a hit test");
    hit.then_some((uid, part))
}

/// What the hit test answers at every point, through the FFI.
unsafe fn hits(model: *mut SimlinModel) -> Vec<Hit> {
    points()
        .into_iter()
        .map(|point| hit_at(model, point))
        .collect()
}

/// What an index built now from the project's datamodel answers at every
/// point.
unsafe fn fresh_hits(proj: *mut SimlinProject) -> Vec<Hit> {
    let datamodel = datamodel_of(proj);
    let index = editing::HitIndex::new(&datamodel, "main").expect("main has a view");
    points()
        .into_iter()
        .map(|(x, y)| {
            index
                .hit(Point::new(x, y), TOLERANCE)
                .map(|h| (h.uid, h.part.into()))
        })
        .collect()
}

/// What the project's DATABASE answers, through entry points that read it:
/// every series of `main`'s run, each variable's equation as the database
/// parsed it, and whether there are diagnostics.
unsafe fn database_answers(proj: *mut SimlinProject) -> String {
    let model = main_model(proj);
    let mut err = ptr::null_mut();
    let sim = simlin_sim_new(model, false, &mut err);
    expect_no_error(err, "creating a simulation");
    simlin_sim_run_to_end(sim, &mut err);
    expect_no_error(err, "running the simulation");
    let mut count = 0;
    simlin_sim_get_var_count(sim, &mut count, &mut err);
    expect_no_error(err, "counting the run's variables");
    let mut names = vec![ptr::null_mut(); count];
    let mut written = 0;
    simlin_sim_get_var_names(sim, names.as_mut_ptr(), names.len(), &mut written, &mut err);
    expect_no_error(err, "naming the run's variables");
    let mut names: Vec<String> = names
        .into_iter()
        .take(written)
        .map(|n| take_string(n))
        .collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let c_name = CString::new(name.as_str()).unwrap();
        let mut values = vec![0.0; 11];
        let mut written = 0;
        simlin_sim_get_series(
            sim,
            c_name.as_ptr(),
            values.as_mut_ptr(),
            values.len(),
            &mut written,
            &mut err,
        );
        expect_no_error(err, "reading a series");
        let latex = take_string(simlin_model_get_latex_equation(
            model,
            c_name.as_ptr(),
            &mut err,
        ));
        expect_no_error(err, "rendering an equation");
        out += &format!("{name}: {:?} = {latex}\n", &values[..written]);
    }
    simlin_sim_unref(sim);
    let errors = simlin_project_get_errors(proj, &mut err);
    expect_no_error(err, "reading the diagnostics");
    out += &format!("diagnostics: {:?}\n", take_error(errors));
    simlin_model_unref(model);
    out
}

/// What the database of a project opened fresh from `proj`'s contents
/// answers.
unsafe fn fresh_database_answers(proj: *mut SimlinProject) -> String {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_project_serialize_protobuf(proj, &mut buf, &mut len, &mut err);
    expect_no_error(err, "serializing the project");
    let fresh = simlin_project_open_protobuf(buf, len, &mut err);
    expect_no_error(err, "opening the serialized project");
    simlin_free(buf);
    let answers = database_answers(fresh);
    simlin_project_unref(fresh);
    answers
}

// ── a row's columns ────────────────────────────────────────────────────

/// How an entry point takes the project's database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Db {
    /// Not at all.
    None,
    /// Alone, once it exists (`SimlinProject::lock_db`): the first query on a
    /// project takes the contents to build it.
    Alone,
    /// With the contents, which it releases before it works on it
    /// (`SimlinProject::lock_contents_and_db`).
    Reader,
    /// As a tool session's entry point takes it: a reader whose wait is not
    /// counted, and is made with the contents released.
    #[cfg_attr(
        not(feature = "agent_tools"),
        expect(dead_code, reason = "the tool session's rows are behind the feature")
    )]
    Call,
    /// Under the contents lock for the whole of its work, building it when
    /// there is none (`SimlinProject::lock_db_with`).
    Writer,
    /// Under the contents lock when one exists, building none
    /// (`SimlinProject::built_db`).
    WhenBuilt,
}

impl Db {
    /// Whether the entry point builds a database on a project with none.
    pub(crate) fn builds(self) -> bool {
        match self {
            Db::Alone | Db::Reader | Db::Call | Db::Writer => true,
            Db::None | Db::WhenBuilt => false,
        }
    }

    /// Whether the entry point counts itself while it waits for the database,
    /// so that a tool call holding it stops.
    fn counted(self) -> bool {
        match self {
            Db::Alone | Db::Reader | Db::Writer | Db::WhenBuilt => true,
            Db::None | Db::Call => false,
        }
    }

    /// What its lock events show on a project whose database exists.
    fn shape(self) -> DbShape {
        match self {
            Db::None => DbShape::Untaken,
            Db::Alone => DbShape::Alone,
            Db::Reader | Db::Call => DbShape::ContentsReleased,
            Db::Writer | Db::WhenBuilt => DbShape::UnderContents,
        }
    }
}

/// The locks an entry point takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Footprint {
    pub session: bool,
    pub contents: bool,
    pub db: Db,
}

/// No project lock: a reference count, a name read off the handle.
const NO_LOCK: Footprint = Footprint {
    session: false,
    contents: false,
    db: Db::None,
};

/// The contents alone.
const CONTENTS: Footprint = Footprint {
    session: false,
    contents: true,
    db: Db::None,
};

/// The database alone, once it exists.
const DB_ALONE: Footprint = Footprint {
    session: false,
    contents: false,
    db: Db::Alone,
};

const fn with_db(db: Db) -> Footprint {
    Footprint {
        session: false,
        contents: true,
        db,
    }
}

#[cfg(feature = "agent_tools")]
const fn session(locks: Footprint) -> Footprint {
    Footprint {
        session: true,
        ..locks
    }
}

/// What an entry point changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Writes {
    Nothing,
    /// `main`'s diagram, and nothing a simulation reads.
    Diagram,
    /// What `main` simulates, and its diagram with it.
    Model,
    /// The project, outside `main`.
    Project,
}

impl Writes {
    /// Whether the change redraws `main`'s diagram, so that a hit index of
    /// the diagram as it was answers some point wrongly.
    fn redraws_main(self) -> bool {
        match self {
            Writes::Diagram | Writes::Model => true,
            Writes::Nothing | Writes::Project => false,
        }
    }
}

/// What a row's runner is handed: the project and its `main`, and the bracket
/// it puts around the entry point itself, so that what a rule observes is the
/// entry point and not the runner's preparation for it.
pub(crate) struct Call<'a> {
    pub proj: *mut SimlinProject,
    pub model: *mut SimlinModel,
    around: &'a dyn Fn(&mut dyn FnMut()),
}

impl Call<'_> {
    fn entry<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        (self.around)(&mut || {
            if let Some(f) = f.take() {
                out = Some(f());
            }
        });
        out.expect("the bracket runs the entry point")
    }
}

/// One entry point, or one of its uses where they take different locks or
/// change different things.
pub(crate) struct EntryPoint {
    /// The exported function the row runs.
    pub name: &'static str,
    /// Which use of it.
    pub case: &'static str,
    pub locks: Footprint,
    pub writes: Writes,
    /// Whether the row runs on the project whose model has no view.
    pub viewless: bool,
    /// Runs the entry point (inside `Call::entry`) and returns what it
    /// answered, as text a rule compares.
    pub run: unsafe fn(&Call) -> String,
}

impl std::fmt::Debug for EntryPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.case.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} ({})", self.name, self.case)
        }
    }
}

impl EntryPoint {
    /// Run the row on `proj` with `around` bracketing the entry point.
    pub(crate) unsafe fn run_on(
        &self,
        proj: *mut SimlinProject,
        around: &dyn Fn(&mut dyn FnMut()),
    ) -> String {
        let model = main_model(proj);
        let answer = (self.run)(&Call {
            proj,
            model,
            around,
        });
        simlin_model_unref(model);
        answer
    }

    /// Run the row on `proj`.
    pub(crate) unsafe fn run_plainly(&self, proj: *mut SimlinProject) -> String {
        self.run_on(proj, &|entry| entry())
    }
}

const fn row(
    name: &'static str,
    case: &'static str,
    locks: Footprint,
    writes: Writes,
    run: unsafe fn(&Call) -> String,
) -> EntryPoint {
    EntryPoint {
        name,
        case,
        locks,
        writes,
        viewless: false,
        run,
    }
}

const fn viewless(row: EntryPoint) -> EntryPoint {
    EntryPoint {
        viewless: true,
        ..row
    }
}

// ── the runners ────────────────────────────────────────────────────────

/// Run an entry point that writes a buffer, and return what it wrote with
/// its error.
unsafe fn buffer(
    call: &Call,
    f: impl FnOnce(*mut *mut u8, *mut usize, *mut *mut SimlinError),
) -> String {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    call.entry(|| f(&mut buf, &mut len, &mut err));
    format!(
        "{} / {:?}",
        String::from_utf8_lossy(&take_bytes(buf, len)),
        take_error(err)
    )
}

fn main_name() -> CString {
    CString::new("main").unwrap()
}

fn press() -> SimlinPress {
    SimlinPress {
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
    }
}

unsafe fn run_project_ref(call: &Call) -> String {
    call.entry(|| {
        simlin_project_ref(call.proj);
        simlin_project_unref(call.proj);
    });
    String::new()
}

unsafe fn run_model_count(call: &Call) -> String {
    let (mut count, mut err) = (0, ptr::null_mut());
    call.entry(|| simlin_project_get_model_count(call.proj, &mut count, &mut err));
    format!("{count} / {:?}", take_error(err))
}

unsafe fn run_revision(call: &Call) -> String {
    let (mut revision, mut err) = (0, ptr::null_mut());
    call.entry(|| simlin_project_get_revision(call.proj, &mut revision, &mut err));
    format!("{:?}", take_error(err))
}

unsafe fn run_model_names(call: &Call) -> String {
    let mut names = vec![ptr::null_mut(); 8];
    let (mut written, mut err) = (0, ptr::null_mut());
    call.entry(|| {
        simlin_project_get_model_names(
            call.proj,
            names.as_mut_ptr(),
            names.len(),
            &mut written,
            &mut err,
        )
    });
    let names: Vec<String> = names
        .into_iter()
        .take(written)
        .map(|n| take_string(n))
        .collect();
    format!("{names:?} / {:?}", take_error(err))
}

unsafe fn run_add_model(call: &Call) -> String {
    let name = CString::new("another").unwrap();
    let mut err = ptr::null_mut();
    call.entry(|| simlin_project_add_model(call.proj, name.as_ptr(), &mut err));
    format!("{:?}", take_error(err))
}

/// An add of a model whose name `main` has, in another spelling: refused,
/// and changing nothing.
unsafe fn run_add_model_taken(call: &Call) -> String {
    let taken = CString::new("Main").unwrap();
    let mut err = ptr::null_mut();
    call.entry(|| simlin_project_add_model(call.proj, taken.as_ptr(), &mut err));
    let refused = take_error(err);
    assert!(refused.is_some(), "`Main` is the model `main`");
    format!("{refused:?}")
}

unsafe fn run_get_model(call: &Call) -> String {
    let name = main_name();
    let mut err = ptr::null_mut();
    let model = call.entry(|| simlin_project_get_model(call.proj, name.as_ptr(), &mut err));
    simlin_model_unref(model);
    format!("{:?}", take_error(err))
}

unsafe fn run_replace_contents(call: &Call) -> String {
    let src = open_json(&project_json(true, 400.0, 400.0, "0.2"));
    let mut err = ptr::null_mut();
    call.entry(|| simlin_project_replace_contents(call.proj, src, &mut err));
    simlin_project_unref(src);
    format!("{:?}", take_error(err))
}

unsafe fn run_replace_contents_with_its_own(call: &Call) -> String {
    let mut err = ptr::null_mut();
    call.entry(|| simlin_project_replace_contents(call.proj, call.proj, &mut err));
    format!("{:?}", take_error(err))
}

unsafe fn run_is_simulatable(call: &Call) -> String {
    let name = main_name();
    let mut err = ptr::null_mut();
    let simulatable =
        call.entry(|| simlin_project_is_simulatable(call.proj, name.as_ptr(), &mut err));
    format!("{simulatable} / {:?}", take_error(err))
}

unsafe fn run_get_errors(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let errors = call.entry(|| simlin_project_get_errors(call.proj, &mut err));
    format!("{:?} / {:?}", take_error(errors), take_error(err))
}

/// Apply `patch` as the row's entry point and return what it collected and
/// what it was rejected with.
unsafe fn patched(call: &Call, patch: &[u8], dry_run: bool, allow_errors: bool) -> String {
    let (mut collected, mut rejected) = (ptr::null_mut(), ptr::null_mut());
    let data = if patch.is_empty() {
        ptr::null()
    } else {
        patch.as_ptr()
    };
    call.entry(|| {
        simlin_project_apply_patch(
            call.proj,
            data,
            patch.len(),
            dry_run,
            allow_errors,
            &mut collected,
            &mut rejected,
        )
    });
    format!("{:?} / {:?}", take_error(collected), take_error(rejected))
}

unsafe fn run_view_patch(call: &Call) -> String {
    patched(
        call,
        &serde_json::to_vec(&move_stock()).unwrap(),
        false,
        false,
    )
}

/// A new aux drawn where nothing was, and `rate` doubled: an edit that
/// validates, changes what `main` simulates, and redraws it.
unsafe fn run_equation_patch(call: &Call) -> String {
    let patch = json!({"models": [{"name": "main", "ops": [
        {"type": "editView", "payload": {"index": 0, "remove": [], "upsert": [
            {"type": "aux", "uid": 10, "name": "fresh", "x": 400.0, "y": 400.0}
        ]}},
        {"type": "upsertAux", "payload": {"aux": {"name": "fresh", "equation": "1"}}},
        {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "0.2"}}}
    ]}]});
    patched(call, &serde_json::to_vec(&patch).unwrap(), false, false)
}

unsafe fn run_dry_run_patch(call: &Call) -> String {
    patched(
        call,
        &serde_json::to_vec(&move_stock()).unwrap(),
        true,
        false,
    )
}

unsafe fn run_rejected_patch(call: &Call) -> String {
    let patch = json!({"models": [{"name": "main", "ops": [
        {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "rate +"}}}
    ]}]});
    let answer = patched(call, &serde_json::to_vec(&patch).unwrap(), false, false);
    assert!(
        !answer.ends_with("/ None"),
        "a patch with an equation error is rejected: {answer}"
    );
    answer
}

unsafe fn run_empty_patch(call: &Call) -> String {
    patched(call, b"", false, false)
}

unsafe fn run_patch_with_no_operations(call: &Call) -> String {
    patched(call, br#"{"projectOps":[],"models":[]}"#, false, false)
}

unsafe fn run_patch_of_a_model_with_no_operations(call: &Call) -> String {
    patched(
        call,
        br#"{"models":[{"name":"main","ops":[]}]}"#,
        false,
        false,
    )
}

unsafe fn run_diagram_sync(call: &Call) -> String {
    let name = main_name();
    let mut err = ptr::null_mut();
    call.entry(|| simlin_project_diagram_sync(call.proj, name.as_ptr(), ptr::null(), &mut err));
    let error = take_error(err);
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_project_serialize_json(call.proj, 0, false, &mut buf, &mut len, &mut err);
    expect_no_error(err, "serializing the project");
    format!(
        "{} / {error:?}",
        String::from_utf8_lossy(&take_bytes(buf, len))
    )
}

unsafe fn run_serialize_protobuf(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_project_serialize_protobuf(call.proj, buf, len, err)
    })
}

unsafe fn run_serialize_json(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_project_serialize_json(call.proj, 0, false, buf, len, err)
    })
}

unsafe fn run_serialize_xmile(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_project_serialize_xmile(call.proj, buf, len, err)
    })
}

unsafe fn run_serialize_mdl(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_project_serialize_mdl(call.proj, buf, len, ptr::null_mut(), err)
    })
}

unsafe fn run_serialize_systems(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_project_serialize_systems(call.proj, buf, len, err)
    })
}

unsafe fn run_check_save(call: &Call) -> String {
    let (mut changes, mut err) = (ptr::null_mut(), ptr::null_mut());
    call.entry(|| {
        simlin_project_check_save(
            call.proj,
            SimlinSaveFormat::Xmile as u32,
            ptr::null(),
            0,
            &mut changes,
            &mut err,
        )
    });
    format!("{:?} / {:?}", take_error(changes), take_error(err))
}

unsafe fn run_render_svg(call: &Call) -> String {
    let name = main_name();
    buffer(call, |buf, len, err| {
        simlin_project_render_svg(call.proj, name.as_ptr(), buf, len, err)
    })
}

unsafe fn run_render_scene(call: &Call) -> String {
    let name = main_name();
    buffer(call, |buf, len, err| {
        simlin_project_render_scene(call.proj, name.as_ptr(), buf, len, err)
    })
}

#[cfg(feature = "png_render")]
unsafe fn run_render_png(call: &Call) -> String {
    let name = main_name();
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    call.entry(|| {
        simlin_project_render_png(
            call.proj,
            name.as_ptr(),
            64,
            0,
            &mut buf,
            &mut len,
            &mut err,
        )
    });
    format!(
        "{} bytes / {:?}",
        take_bytes(buf, len).len(),
        take_error(err)
    )
}

unsafe fn run_model_ref(call: &Call) -> String {
    call.entry(|| {
        simlin_model_ref(call.model);
        simlin_model_unref(call.model);
    });
    String::new()
}

unsafe fn run_model_name(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let name = call.entry(|| simlin_model_get_name(call.model, &mut err));
    format!("{} / {:?}", take_string(name), take_error(err))
}

unsafe fn run_var_count(call: &Call) -> String {
    let (mut count, mut err) = (0, ptr::null_mut());
    call.entry(|| simlin_model_get_var_count(call.model, 0, ptr::null(), &mut count, &mut err));
    format!("{count} / {:?}", take_error(err))
}

unsafe fn run_var_names(call: &Call) -> String {
    let mut names = vec![ptr::null_mut(); 8];
    let (mut written, mut err) = (0, ptr::null_mut());
    call.entry(|| {
        simlin_model_get_var_names(
            call.model,
            0,
            ptr::null(),
            names.as_mut_ptr(),
            names.len(),
            &mut written,
            &mut err,
        )
    });
    let names: Vec<String> = names
        .into_iter()
        .take(written)
        .map(|n| take_string(n))
        .collect();
    format!("{names:?} / {:?}", take_error(err))
}

unsafe fn run_var_json(call: &Call) -> String {
    let births = CString::new("births").unwrap();
    buffer(call, |buf, len, err| {
        simlin_model_get_var_json(call.model, births.as_ptr(), buf, len, err)
    })
}

unsafe fn run_sim_specs_json(call: &Call) -> String {
    buffer(call, |buf, len, err| {
        simlin_model_get_sim_specs_json(call.model, buf, len, err)
    })
}

unsafe fn run_incoming_links(call: &Call) -> String {
    let births = CString::new("births").unwrap();
    let mut names = vec![ptr::null_mut(); 8];
    let (mut written, mut err) = (0, ptr::null_mut());
    call.entry(|| {
        simlin_model_get_incoming_links(
            call.model,
            births.as_ptr(),
            names.as_mut_ptr(),
            names.len(),
            &mut written,
            &mut err,
        )
    });
    let names: Vec<String> = names
        .into_iter()
        .take(written)
        .map(|n| take_string(n))
        .collect();
    format!("{names:?} / {:?}", take_error(err))
}

/// The links as sorted `(from, to)` pairs, freeing them.
unsafe fn link_pairs(links: *mut SimlinLinks) -> Vec<(String, String)> {
    if links.is_null() {
        return Vec::new();
    }
    let list = std::slice::from_raw_parts((*links).links, (*links).count);
    let mut pairs: Vec<(String, String)> = list
        .iter()
        .map(|l| {
            (
                CStr::from_ptr(l.from).to_string_lossy().into_owned(),
                CStr::from_ptr(l.to).to_string_lossy().into_owned(),
            )
        })
        .collect();
    simlin_free_links(links);
    pairs.sort();
    pairs
}

/// The loops' ids, freeing them.
unsafe fn loop_ids(loops: *mut SimlinLoops) -> Vec<String> {
    if loops.is_null() {
        return Vec::new();
    }
    let list = std::slice::from_raw_parts((*loops).loops, (*loops).count);
    let ids = list
        .iter()
        .map(|l| CStr::from_ptr(l.id).to_string_lossy().into_owned())
        .collect();
    simlin_free_loops(loops);
    ids
}

unsafe fn run_model_links(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let links = call.entry(|| simlin_model_get_links(call.model, &mut err));
    format!("{:?} / {:?}", link_pairs(links), take_error(err))
}

unsafe fn run_latex_equation(call: &Call) -> String {
    let births = CString::new("births").unwrap();
    let mut err = ptr::null_mut();
    let latex =
        call.entry(|| simlin_model_get_latex_equation(call.model, births.as_ptr(), &mut err));
    format!("{} / {:?}", take_string(latex), take_error(err))
}

/// The model compiled to wasm: the blob and its layout.
unsafe fn compiled_to_wasm(
    model: *mut SimlinModel,
    ltm: bool,
    err: &mut *mut SimlinError,
) -> (Vec<u8>, Vec<u8>) {
    let (mut wasm, mut wasm_len) = (ptr::null_mut(), 0);
    let (mut layout, mut layout_len) = (ptr::null_mut(), 0);
    simlin_model_compile_to_wasm(
        model,
        ltm,
        false,
        &mut wasm,
        &mut wasm_len,
        &mut layout,
        &mut layout_len,
        err,
    );
    (take_bytes(wasm, wasm_len), take_bytes(layout, layout_len))
}

unsafe fn run_compile_to_wasm(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let (wasm, layout) = call.entry(|| compiled_to_wasm(call.model, false, &mut err));
    // The layout's name map is compared as a map: its entries come out in
    // the order of the compiled program's offset map, a `HashMap`, which
    // differs between two databases.
    let layout = simlin_engine::wasmgen::WasmLayout::deserialize(&layout).map(|layout| {
        let mut names = layout.var_offsets;
        names.sort();
        (
            layout.n_slots,
            layout.n_chunks,
            layout.results_offset,
            names,
        )
    });
    format!("{wasm:?} {layout:?} / {:?}", take_error(err))
}

unsafe fn run_sim_new(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let sim = call.entry(|| simlin_sim_new(call.model, false, &mut err));
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
    format!("{:?} / {:?}", &values[..written], take_error(err))
}

unsafe fn run_structural_loops(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let loops = call.entry(|| simlin_analyze_get_loops(call.model, &mut err));
    format!("{:?} / {:?}", loop_ids(loops), take_error(err))
}

unsafe fn run_discover_loops(call: &Call) -> String {
    let mut err = ptr::null_mut();
    let result = call.entry(|| simlin_analyze_discover_loops(call.model, 0, &mut err));
    let count = if result.is_null() {
        None
    } else {
        let count = (*result).loop_count;
        simlin_free_discovery_result(result);
        Some(count)
    };
    format!("{count:?} / {:?}", take_error(err))
}

/// A wasm layout of the fixture compiled under LTM, and a slab of zeros its
/// size: what the from-wasm analyses read. Compiled on a project of its own,
/// so the row's project is as it was when its entry point runs.
unsafe fn wasm_results() -> (Vec<u8>, Vec<u8>) {
    let other = open(true);
    let model = main_model(other);
    let mut err = ptr::null_mut();
    let (_, layout) = compiled_to_wasm(model, true, &mut err);
    expect_no_error(err, "compiling to wasm");
    simlin_model_unref(model);
    simlin_project_unref(other);
    let parsed = simlin_engine::wasmgen::WasmLayout::deserialize(&layout).expect("a wasm layout");
    let slab = vec![0u8; parsed.n_chunks * parsed.n_slots * 8];
    (slab, layout)
}

unsafe fn run_links_from_wasm(call: &Call) -> String {
    let (slab, layout) = wasm_results();
    let mut err = ptr::null_mut();
    let links = call.entry(|| {
        simlin_analyze_links_from_wasm_results(
            call.model,
            slab.as_ptr(),
            slab.len(),
            layout.as_ptr(),
            layout.len(),
            false,
            &mut err,
        )
    });
    format!("{:?} / {:?}", link_pairs(links), take_error(err))
}

unsafe fn run_rel_loop_score_from_wasm(call: &Call) -> String {
    let (slab, layout) = wasm_results();
    let loop_id = CString::new("r1").unwrap();
    let mut scores = vec![0.0; 11];
    let (mut written, mut err) = (0, ptr::null_mut());
    call.entry(|| {
        simlin_analyze_rel_loop_score_from_wasm_results(
            call.model,
            slab.as_ptr(),
            slab.len(),
            layout.as_ptr(),
            layout.len(),
            loop_id.as_ptr(),
            scores.as_mut_ptr(),
            scores.len(),
            &mut written,
            &mut err,
        )
    });
    format!("{written} / {:?}", take_error(err))
}

/// A simulation of `main` under LTM, run to its end: what the analyses of a
/// run read.
unsafe fn run_sim(model: *mut SimlinModel) -> *mut SimlinSim {
    let mut err = ptr::null_mut();
    let sim = simlin_sim_new(model, true, &mut err);
    expect_no_error(err, "creating a simulation");
    simlin_sim_run_to_end(sim, &mut err);
    expect_no_error(err, "running the simulation");
    sim
}

unsafe fn run_loops_runtime(call: &Call) -> String {
    let sim = run_sim(call.model);
    let mut err = ptr::null_mut();
    let loops = call.entry(|| simlin_analyze_get_loops_runtime(sim, &mut err));
    simlin_sim_unref(sim);
    format!("{:?} / {:?}", loop_ids(loops), take_error(err))
}

unsafe fn run_sim_links(call: &Call) -> String {
    let sim = run_sim(call.model);
    let mut err = ptr::null_mut();
    let links = call.entry(|| simlin_analyze_get_links(sim, false, &mut err));
    simlin_sim_unref(sim);
    format!("{:?} / {:?}", link_pairs(links), take_error(err))
}

unsafe fn run_hit_test(call: &Call) -> String {
    let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
    let mut err = ptr::null_mut();
    call.entry(|| {
        simlin_model_hit_test(
            call.model, 100.0, 100.0, TOLERANCE, &mut hit, &mut uid, &mut part, &mut err,
        )
    });
    format!("{hit} {uid} / {:?}", take_error(err))
}

unsafe fn run_plan_tap(call: &Call) -> String {
    let press = press();
    buffer(call, |buf, len, err| {
        simlin_model_plan_tap(call.model, &press, buf, len, err)
    })
}

unsafe fn run_gesture_begin(call: &Call) -> String {
    let press = press();
    let mut err = ptr::null_mut();
    let gesture = call.entry(|| simlin_gesture_begin(call.model, &press, &mut err));
    simlin_gesture_unref(gesture);
    format!("{:?}", take_error(err))
}

unsafe fn run_plan_move(call: &Call) -> String {
    let selection = [STOCK];
    buffer(call, |buf, len, err| {
        simlin_model_plan_move(call.model, selection.as_ptr(), 1, 10.0, 0.0, buf, len, err)
    })
}

unsafe fn run_plan_delete(call: &Call) -> String {
    let selection = [STOCK];
    buffer(call, |buf, len, err| {
        simlin_model_plan_delete(call.model, selection.as_ptr(), 1, buf, len, err)
    })
}

unsafe fn run_plan_rename(call: &Call) -> String {
    let (from, to) = (
        CString::new("rate").unwrap(),
        CString::new("growth").unwrap(),
    );
    buffer(call, |buf, len, err| {
        simlin_model_plan_rename(call.model, from.as_ptr(), to.as_ptr(), buf, len, err)
    })
}

#[cfg(feature = "agent_tools")]
mod tool_rows {
    use super::*;

    unsafe fn new_session(model: *mut SimlinModel) -> *mut SimlinToolSession {
        let mut err = ptr::null_mut();
        let session = simlin_tool_session_new(model, &mut err);
        expect_no_error(err, "making a tool session");
        session
    }

    /// Call `tool` on `session` and return whether it refused, and its
    /// output.
    unsafe fn tool_call(
        session: *mut SimlinToolSession,
        tool: &str,
        input: &str,
    ) -> (bool, String) {
        let name = CString::new(tool).unwrap();
        let (mut buf, mut len, mut is_error, mut err) =
            (ptr::null_mut(), 0, false, ptr::null_mut());
        simlin_tool_session_call(
            session,
            name.as_ptr(),
            input.as_ptr(),
            input.len(),
            &mut buf,
            &mut len,
            &mut is_error,
            &mut err,
        );
        expect_no_error(err, tool);
        (
            is_error,
            String::from_utf8_lossy(&take_bytes(buf, len)).into_owned(),
        )
    }

    /// An edit whose gate passes: a new variable `rate` reads, at twice the
    /// rate, so it changes what `main` simulates and, laid out, its diagram.
    fn an_edit() -> String {
        json!({"summary": "a named growth rate", "operations": [
            {"op": "add_variable", "name": "base_rate", "equation": "0.2"},
            {"op": "set_equation", "variable": "rate", "equation": "base_rate"}
        ]})
        .to_string()
    }

    /// An edit whose gate refuses it: it gives the model an error it did not
    /// have.
    fn a_refused_edit() -> String {
        json!({"summary": "a broken growth rate", "operations": [
            {"op": "set_equation", "variable": "rate", "equation": "nothing_here * 2"}
        ]})
        .to_string()
    }

    /// A session over `model` that has read it, as an edit requires.
    unsafe fn read_session(model: *mut SimlinModel) -> *mut SimlinToolSession {
        let session = new_session(model);
        let (refused, outline) = tool_call(session, "read_model", "{}");
        assert!(!refused, "{outline}");
        session
    }

    pub(super) unsafe fn run_session_new(call: &Call) -> String {
        let mut err = ptr::null_mut();
        let session = call.entry(|| simlin_tool_session_new(call.model, &mut err));
        simlin_tool_session_unref(session);
        format!("{:?}", take_error(err))
    }

    pub(super) unsafe fn run_session_ref(call: &Call) -> String {
        let session = new_session(call.model);
        call.entry(|| {
            simlin_tool_session_ref(session);
            simlin_tool_session_unref(session);
        });
        simlin_tool_session_unref(session);
        String::new()
    }

    pub(super) unsafe fn run_call(call: &Call) -> String {
        let session = new_session(call.model);
        let (refused, output) = call.entry(|| tool_call(session, "read_model", "{}"));
        simlin_tool_session_unref(session);
        format!("{refused} {output}")
    }

    /// A tool that edits makes its edit in the call, when its gate passes.
    pub(super) unsafe fn run_call_that_edits(call: &Call) -> String {
        let session = read_session(call.model);
        let (refused, output) = call.entry(|| tool_call(session, "edit_model", &an_edit()));
        simlin_tool_session_unref(session);
        assert!(!refused, "the edit is made: {output}");
        format!("{refused} {output}")
    }

    /// An edit the gate refuses stages on the database, restores it, and
    /// changes nothing.
    pub(super) unsafe fn run_call_whose_edit_is_refused(call: &Call) -> String {
        let session = read_session(call.model);
        let (refused, output) = call.entry(|| tool_call(session, "edit_model", &a_refused_edit()));
        simlin_tool_session_unref(session);
        assert!(refused, "the gate's refusal is a refusal: {output}");
        format!("{refused} {output}")
    }

    pub(super) unsafe fn run_cancel(call: &Call) -> String {
        let session = new_session(call.model);
        call.entry(|| simlin_tool_session_cancel(session));
        simlin_tool_session_unref(session);
        String::new()
    }

    pub(super) unsafe fn run_forget_run(call: &Call) -> String {
        let session = new_session(call.model);
        let name = CString::new("nowhere").unwrap();
        let (mut forgotten, mut err) = (true, ptr::null_mut());
        call.entry(|| {
            simlin_tool_session_forget_run(session, name.as_ptr(), &mut forgotten, &mut err)
        });
        simlin_tool_session_unref(session);
        format!("{forgotten} / {:?}", take_error(err))
    }

    pub(super) unsafe fn run_get_changes(call: &Call) -> String {
        let session = new_session(call.model);
        let answer = buffer(call, |buf, len, err| {
            simlin_tool_session_get_changes(session, buf, len, err)
        });
        simlin_tool_session_unref(session);
        answer
    }

    pub(super) unsafe fn run_get_run(call: &Call) -> String {
        let session = new_session(call.model);
        let current = CString::new("current").unwrap();
        let mut err = ptr::null_mut();
        let results = call.entry(|| {
            simlin_tool_session_get_run(
                session,
                current.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut err,
            )
        });
        simlin_tool_session_unref(session);
        let error = take_error(err);
        let population = CString::new("population").unwrap();
        let mut values = vec![0.0; 11];
        let (mut written, mut series_err) = (0, ptr::null_mut());
        if !results.is_null() {
            simlin_results_get_series(
                results,
                population.as_ptr(),
                values.as_mut_ptr(),
                values.len(),
                &mut written,
                &mut series_err,
            );
            simlin_results_unref(results);
        }
        format!(
            "{:?} / {error:?} {:?}",
            &values[..written],
            take_error(series_err)
        )
    }

    /// A run an experiment made, which the session keeps: read with no
    /// database.
    pub(super) unsafe fn run_get_kept_run(call: &Call) -> String {
        let session = read_session(call.model);
        let (refused, made) = tool_call(
            session,
            "run_experiment",
            &json!({"name": "faster", "set": [{"variable": "rate", "value": 0.2}]}).to_string(),
        );
        assert!(!refused, "{made}");
        let name = CString::new("faster").unwrap();
        let mut err = ptr::null_mut();
        let results = call.entry(|| {
            simlin_tool_session_get_run(
                session,
                name.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut err,
            )
        });
        simlin_tool_session_unref(session);
        let error = take_error(err);
        let kept = !results.is_null();
        if kept {
            simlin_results_unref(results);
        }
        format!("{kept} / {error:?}")
    }

    pub(super) unsafe fn run_list_runs(call: &Call) -> String {
        let session = new_session(call.model);
        let answer = buffer(call, |buf, len, err| {
            simlin_tool_session_list_runs(session, buf, len, err)
        });
        simlin_tool_session_unref(session);
        answer
    }
}

#[cfg(feature = "agent_tools")]
use tool_rows::*;

// ── the table ──────────────────────────────────────────────────────────

/// Every entry point that reaches a project.
pub(crate) const ALL: &[EntryPoint] = &[
    row(
        "simlin_project_ref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_project_ref,
    ),
    row(
        "simlin_project_unref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_project_ref,
    ),
    row(
        "simlin_project_get_model_count",
        "",
        CONTENTS,
        Writes::Nothing,
        run_model_count,
    ),
    row(
        "simlin_project_get_revision",
        "",
        CONTENTS,
        Writes::Nothing,
        run_revision,
    ),
    row(
        "simlin_project_get_model_names",
        "",
        CONTENTS,
        Writes::Nothing,
        run_model_names,
    ),
    row(
        "simlin_project_add_model",
        "",
        with_db(Db::WhenBuilt),
        Writes::Project,
        run_add_model,
    ),
    row(
        "simlin_project_add_model",
        "a name another model has",
        CONTENTS,
        Writes::Nothing,
        run_add_model_taken,
    ),
    row(
        "simlin_project_get_model",
        "",
        CONTENTS,
        Writes::Nothing,
        run_get_model,
    ),
    row(
        "simlin_project_replace_contents",
        "another project's contents",
        with_db(Db::WhenBuilt),
        Writes::Model,
        run_replace_contents,
    ),
    row(
        "simlin_project_replace_contents",
        "the contents it holds",
        CONTENTS,
        Writes::Nothing,
        run_replace_contents_with_its_own,
    ),
    row(
        "simlin_project_is_simulatable",
        "",
        with_db(Db::Reader),
        Writes::Nothing,
        run_is_simulatable,
    ),
    row(
        "simlin_project_get_errors",
        "",
        with_db(Db::Reader),
        Writes::Nothing,
        run_get_errors,
    ),
    row(
        "simlin_project_apply_patch",
        "a diagram edit",
        CONTENTS,
        Writes::Diagram,
        run_view_patch,
    ),
    row(
        "simlin_project_apply_patch",
        "an edit that validates",
        with_db(Db::Writer),
        Writes::Model,
        run_equation_patch,
    ),
    row(
        "simlin_project_apply_patch",
        "a dry run",
        with_db(Db::Writer),
        Writes::Nothing,
        run_dry_run_patch,
    ),
    row(
        "simlin_project_apply_patch",
        "a rejected edit",
        with_db(Db::Writer),
        Writes::Nothing,
        run_rejected_patch,
    ),
    row(
        "simlin_project_apply_patch",
        "an empty patch",
        CONTENTS,
        Writes::Nothing,
        run_empty_patch,
    ),
    row(
        "simlin_project_apply_patch",
        "a patch with no operations",
        CONTENTS,
        Writes::Nothing,
        run_patch_with_no_operations,
    ),
    row(
        "simlin_project_apply_patch",
        "a patch of a model with no operations",
        CONTENTS,
        Writes::Nothing,
        run_patch_of_a_model_with_no_operations,
    ),
    row(
        "simlin_project_diagram_sync",
        "",
        with_db(Db::Writer),
        Writes::Diagram,
        run_diagram_sync,
    ),
    row(
        "simlin_project_serialize_protobuf",
        "",
        CONTENTS,
        Writes::Nothing,
        run_serialize_protobuf,
    ),
    row(
        "simlin_project_serialize_json",
        "",
        CONTENTS,
        Writes::Nothing,
        run_serialize_json,
    ),
    row(
        "simlin_project_serialize_xmile",
        "",
        CONTENTS,
        Writes::Nothing,
        run_serialize_xmile,
    ),
    row(
        "simlin_project_serialize_mdl",
        "",
        CONTENTS,
        Writes::Nothing,
        run_serialize_mdl,
    ),
    row(
        "simlin_project_serialize_systems",
        "",
        CONTENTS,
        Writes::Nothing,
        run_serialize_systems,
    ),
    row(
        "simlin_project_check_save",
        "",
        CONTENTS,
        Writes::Nothing,
        run_check_save,
    ),
    row(
        "simlin_project_render_svg",
        "a model with a view",
        CONTENTS,
        Writes::Nothing,
        run_render_svg,
    ),
    viewless(row(
        "simlin_project_render_svg",
        "a model with no view",
        with_db(Db::Reader),
        Writes::Nothing,
        run_render_svg,
    )),
    row(
        "simlin_project_render_scene",
        "a model with a view",
        CONTENTS,
        Writes::Nothing,
        run_render_scene,
    ),
    viewless(row(
        "simlin_project_render_scene",
        "a model with no view",
        with_db(Db::Reader),
        Writes::Nothing,
        run_render_scene,
    )),
    #[cfg(feature = "png_render")]
    row(
        "simlin_project_render_png",
        "",
        CONTENTS,
        Writes::Nothing,
        run_render_png,
    ),
    row(
        "simlin_model_ref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_model_ref,
    ),
    row(
        "simlin_model_unref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_model_ref,
    ),
    row(
        "simlin_model_get_name",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_model_name,
    ),
    row(
        "simlin_model_get_var_count",
        "",
        CONTENTS,
        Writes::Nothing,
        run_var_count,
    ),
    row(
        "simlin_model_get_var_names",
        "",
        CONTENTS,
        Writes::Nothing,
        run_var_names,
    ),
    row(
        "simlin_model_get_var_json",
        "",
        CONTENTS,
        Writes::Nothing,
        run_var_json,
    ),
    row(
        "simlin_model_get_sim_specs_json",
        "",
        CONTENTS,
        Writes::Nothing,
        run_sim_specs_json,
    ),
    row(
        "simlin_model_get_incoming_links",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_incoming_links,
    ),
    row(
        "simlin_model_get_links",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_model_links,
    ),
    row(
        "simlin_model_get_latex_equation",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_latex_equation,
    ),
    row(
        "simlin_model_compile_to_wasm",
        "",
        with_db(Db::Reader),
        Writes::Nothing,
        run_compile_to_wasm,
    ),
    row(
        "simlin_sim_new",
        "",
        with_db(Db::Reader),
        Writes::Nothing,
        run_sim_new,
    ),
    row(
        "simlin_analyze_get_loops",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_structural_loops,
    ),
    row(
        "simlin_analyze_discover_loops",
        "",
        with_db(Db::Reader),
        Writes::Nothing,
        run_discover_loops,
    ),
    row(
        "simlin_analyze_links_from_wasm_results",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_links_from_wasm,
    ),
    row(
        "simlin_analyze_rel_loop_score_from_wasm_results",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_rel_loop_score_from_wasm,
    ),
    row(
        "simlin_analyze_get_loops_runtime",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_loops_runtime,
    ),
    row(
        "simlin_analyze_get_links",
        "",
        DB_ALONE,
        Writes::Nothing,
        run_sim_links,
    ),
    row(
        "simlin_model_hit_test",
        "",
        CONTENTS,
        Writes::Nothing,
        run_hit_test,
    ),
    row(
        "simlin_model_plan_tap",
        "",
        CONTENTS,
        Writes::Nothing,
        run_plan_tap,
    ),
    row(
        "simlin_gesture_begin",
        "",
        CONTENTS,
        Writes::Nothing,
        run_gesture_begin,
    ),
    row(
        "simlin_model_plan_move",
        "",
        CONTENTS,
        Writes::Nothing,
        run_plan_move,
    ),
    row(
        "simlin_model_plan_delete",
        "",
        CONTENTS,
        Writes::Nothing,
        run_plan_delete,
    ),
    row(
        "simlin_model_plan_rename",
        "",
        CONTENTS,
        Writes::Nothing,
        run_plan_rename,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_new",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_session_new,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_ref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_session_ref,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_unref",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_session_ref,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_call",
        "a tool that reads",
        session(with_db(Db::Call)),
        Writes::Nothing,
        run_call,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_call",
        "a tool that edits",
        session(with_db(Db::Writer)),
        Writes::Model,
        run_call_that_edits,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_call",
        "an edit its gate refuses",
        session(with_db(Db::Writer)),
        Writes::Nothing,
        run_call_whose_edit_is_refused,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_cancel",
        "",
        NO_LOCK,
        Writes::Nothing,
        run_cancel,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_forget_run",
        "",
        session(NO_LOCK),
        Writes::Nothing,
        run_forget_run,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_get_changes",
        "",
        session(CONTENTS),
        Writes::Nothing,
        run_get_changes,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_get_run",
        "a run it must make",
        session(with_db(Db::Call)),
        Writes::Nothing,
        run_get_run,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_get_run",
        "a run it keeps",
        session(CONTENTS),
        Writes::Nothing,
        run_get_kept_run,
    ),
    #[cfg(feature = "agent_tools")]
    row(
        "simlin_tool_session_list_runs",
        "",
        session(CONTENTS),
        Writes::Nothing,
        run_list_runs,
    ),
];

// ── every entry point has a row ────────────────────────────────────────

/// Every function the C header declares, with its parameter list. The header
/// is cbindgen's output, which the pre-commit hook and CI hold fresh against
/// the crate, so a function the crate exports is here whatever spelling its
/// export attribute has (`#[no_mangle]`, `#[unsafe(no_mangle)]`,
/// `#[export_name]`), whatever its ABI string, and whichever file defines it;
/// cbindgen emits a feature-gated function unconditionally (the header is
/// the full native API).
fn exported(header: &str) -> Vec<(String, String)> {
    let code: String = header
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !line.starts_with("//") && !line.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = Vec::new();
    for statement in code.split(';') {
        // What follows a struct's or an `extern "C"` block's brace.
        let statement = statement.rsplit(['{', '}']).next().unwrap_or_default();
        if statement.contains("typedef") {
            continue;
        }
        let Some(at) = statement.find("simlin_") else {
            continue;
        };
        let rest = &statement[at..];
        let Some(open) = rest.find('(') else {
            continue;
        };
        let name = &rest[..open];
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        // The parameter list runs to the parenthesis that closes it, past a
        // function-pointer parameter's own.
        let mut depth = 0;
        let close = rest[open..]
            .char_indices()
            .find(|&(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map(|(i, _)| open + i)
            .expect("a declaration's parameter list closes");
        out.push((name.to_string(), rest[open + 1..close].to_string()));
    }
    out
}

/// Whether this build compiles `name`: the header declares the functions of
/// every feature, and a row of one this build leaves out is left out too.
fn compiled(name: &str) -> bool {
    // (whether a feature gates the function, whether this build has it)
    let gates = [
        (
            name.starts_with("simlin_tool_session_") || name == "simlin_tools_describe",
            cfg!(feature = "agent_tools"),
        ),
        (
            name == "simlin_project_render_png",
            cfg!(feature = "png_render"),
        ),
    ];
    gates.iter().all(|&(gated, built)| !gated || built)
}

/// The handles through which an entry point is given a project.
const PROJECT_HANDLES: [&str; 3] = ["SimlinProject", "SimlinModel", "SimlinToolSession"];

/// The exported functions that reach no project, each with why. A function
/// here takes none of `PROJECT_HANDLES`, which the scan checks; that one
/// handed a simulation, a gesture or a results table works on what that
/// handle holds alone, and never on the project behind it, is the reason's
/// claim, which no test observes.
const REACHES_NO_PROJECT: &[(&str, &str)] = &[
    ("simlin_free_loops", "frees a loop list it is handed"),
    (
        "simlin_free_discovery_result",
        "frees a discovery result it is handed",
    ),
    ("simlin_free_links", "frees a link list it is handed"),
    (
        "simlin_sim_get_ltm_mode",
        "reads the mode the simulation captured when it was made",
    ),
    (
        "simlin_analyze_get_relative_loop_score",
        "reads the simulation's results and its own LTM snapshot",
    ),
    (
        "simlin_analyze_get_rel_loop_score",
        "reads the simulation's results and its own LTM snapshot",
    ),
    (
        "simlin_analyze_get_loop_score",
        "reads the simulation's results and its own LTM snapshot",
    ),
    (
        "simlin_analyze_get_loop_element_count",
        "reads the simulation's own LTM snapshot",
    ),
    (
        "simlin_gesture_frame",
        "plans against the gesture's own copy of the view",
    ),
    (
        "simlin_gesture_commit",
        "plans against the gesture's own copy of the view",
    ),
    ("simlin_gesture_ref", "a reference count"),
    (
        "simlin_gesture_unref",
        "a reference count, and the gesture's own copy",
    ),
    ("simlin_error_str", "names an error code"),
    ("simlin_sizeof_loop", "a struct's size"),
    ("simlin_sizeof_link", "a struct's size"),
    ("simlin_sizeof_error_detail", "a struct's size"),
    ("simlin_sizeof_ptr", "a pointer's size"),
    ("simlin_error_free", "frees an error"),
    ("simlin_error_get_code", "reads an error"),
    ("simlin_error_get_message", "reads an error"),
    ("simlin_error_get_detail_count", "reads an error"),
    ("simlin_error_get_details", "reads an error"),
    ("simlin_error_get_detail", "reads an error"),
    ("simlin_malloc", "allocates"),
    ("simlin_free", "frees a buffer"),
    ("simlin_free_string", "frees a string"),
    ("simlin_init", "installs the panic hook"),
    ("simlin_get_panic_message", "reads the last panic's message"),
    (
        "simlin_clear_panic_message",
        "clears the last panic's message",
    ),
    (
        "simlin_project_open_protobuf",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_open_json",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_new",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_open_xmile",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_open_vensim",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_open_vensim_with_data",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_project_open_systems",
        "makes a new project, which no one else holds yet",
    ),
    (
        "simlin_import_losses",
        "reads a file's bytes and makes no project",
    ),
    (
        "simlin_results_open_vdf",
        "reads a VDF's bytes into a results table",
    ),
    ("simlin_results_ref", "a reference count"),
    ("simlin_results_unref", "a reference count, and the table"),
    ("simlin_results_get_stepcount", "reads a results table"),
    ("simlin_results_get_var_count", "reads a results table"),
    ("simlin_results_get_var_names", "reads a results table"),
    ("simlin_results_get_series", "reads a results table"),
    ("simlin_sim_ref", "a reference count"),
    (
        "simlin_sim_unref",
        "a reference count; the last drops the simulation's model reference, a count too",
    ),
    (
        "simlin_sim_run_to",
        "runs the simulation's own compiled program",
    ),
    (
        "simlin_sim_run_to_end",
        "runs the simulation's own compiled program",
    ),
    (
        "simlin_sim_get_stepcount",
        "reads the simulation's own state",
    ),
    ("simlin_sim_reset", "resets the simulation's own state"),
    (
        "simlin_sim_run_initials",
        "runs the simulation's own compiled program",
    ),
    ("simlin_sim_get_value", "reads the simulation's own state"),
    ("simlin_sim_set_value", "sets the simulation's own state"),
    (
        "simlin_sim_clear_values",
        "clears the simulation's own overrides",
    ),
    (
        "simlin_sim_set_value_by_offset",
        "sets the simulation's own results",
    ),
    (
        "simlin_sim_get_offset",
        "reads the simulation's own program",
    ),
    (
        "simlin_sim_get_var_count",
        "reads the simulation's own program",
    ),
    (
        "simlin_sim_get_var_names",
        "reads the simulation's own program",
    ),
    (
        "simlin_sim_get_series",
        "reads the simulation's own results",
    ),
    ("simlin_tools_describe", "writes the engine's catalog"),
];

/// The functions `header` exports that this build compiles and that neither
/// have a row in `rows` nor are said to reach no project.
fn unclassified(header: &str, rows: &[&str]) -> Vec<String> {
    exported(header)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| compiled(name))
        .filter(|name| {
            !rows.contains(&name.as_str())
                && !REACHES_NO_PROJECT.iter().any(|(listed, _)| listed == name)
        })
        .collect()
}

/// The functions `REACHES_NO_PROJECT` lists that `header` declares taking a
/// project, a model or a tool session: each a reason the scan refutes.
fn handed_a_project(header: &str) -> Vec<String> {
    let exported = exported(header);
    REACHES_NO_PROJECT
        .iter()
        .filter(|(name, _)| {
            exported
                .iter()
                .find(|(exported, _)| exported == name)
                .is_some_and(|(_, params)| {
                    PROJECT_HANDLES.iter().any(|handle| params.contains(handle))
                })
        })
        .map(|(name, _)| name.to_string())
        .collect()
}

fn row_names() -> Vec<&'static str> {
    ALL.iter().map(|row| row.name).collect()
}

#[test]
fn every_exported_function_has_a_row_or_reaches_no_project() {
    let header = include_str!("../simlin.h");
    let exported = exported(header);
    assert!(
        exported.len() > 100,
        "the scan reads the header's functions: it found {}",
        exported.len()
    );
    let missing = unclassified(header, &row_names());
    assert!(
        missing.is_empty(),
        "exported functions with neither a row in `entry_point_tests::ALL` nor a reason in \
         `REACHES_NO_PROJECT`:\n{}",
        missing.join("\n")
    );
    assert_eq!(
        handed_a_project(header),
        Vec::<String>::new(),
        "functions said to reach no project that are handed one"
    );
    for (name, reason) in REACHES_NO_PROJECT {
        assert!(
            exported.iter().any(|(exported, _)| exported == name),
            "{name} ({reason}) is no exported function"
        );
        assert!(
            !ALL.iter().any(|row| row.name == *name),
            "{name} has a row and is said to reach no project"
        );
    }
    let stale: Vec<&str> = ALL
        .iter()
        .map(|row| row.name)
        .filter(|name| !exported.iter().any(|(exported, _)| exported == name))
        .collect();
    assert!(
        stale.is_empty(),
        "rows that name no exported function: {stale:?}"
    );
}

/// The scan finds an entry point however it is added: each row is a way to
/// add one that locks a project (the declaration cbindgen writes for it,
/// beside the header's own), and each is reported until it has a row. Read
/// the header, never the Rust sources: a source scan misses seven of these
/// nine (the `#[unsafe(no_mangle)]` and `#[export_name]` spellings, `extern
/// "C-unwind"`, a function-pointer parameter before the handle, a
/// simulation handle that reaches the project through a helper, and an
/// entry point in `lib.rs` or in a new file).
#[test]
fn the_scan_reports_an_entry_point_however_it_is_added() {
    let header = include_str!("../simlin.h");
    let added = [
        // `#[no_mangle]`, in a file the crate has.
        "uint64_t simlin_added_plain(SimlinProject *project);",
        // `#[unsafe(no_mangle)]`, edition 2024's spelling.
        "uint64_t simlin_added_unsafe_attr(SimlinProject *project);",
        // `#[export_name = "..."]` on a function of another name.
        "uint64_t simlin_added_export_name(SimlinProject *project);",
        // Behind a feature.
        "uint64_t simlin_added_cfg(SimlinProject *project);",
        // `extern "C-unwind"`.
        "uint64_t simlin_added_c_unwind(SimlinProject *project);",
        // A function-pointer parameter before the handle.
        "uint64_t simlin_added_fnptr(uint32_t (*cb)(uint32_t), SimlinProject *project);",
        // A simulation handle, the project reached through a helper.
        "uint64_t simlin_added_sim_helper(SimlinSim *sim);",
        // Defined in `lib.rs`.
        "uint64_t simlin_added_in_lib(SimlinProject *project);",
        // Defined in a new source file.
        "uint64_t simlin_added_new_file(SimlinProject *project);",
    ];
    for declaration in added {
        let name =
            &declaration[declaration.find("simlin_").unwrap()..declaration.find('(').unwrap()];
        let with = format!("{header}\n{declaration}\n");
        assert_eq!(
            unclassified(&with, &row_names()),
            vec![name.to_string()],
            "{declaration}"
        );
        let params = exported(&with)
            .into_iter()
            .find(|(exported, _)| exported == name)
            .map(|(_, params)| params)
            .unwrap();
        assert!(
            declaration.ends_with(&format!("({params});")),
            "the whole parameter list: {params}"
        );
        let mut rows = row_names();
        rows.push(name);
        assert!(
            unclassified(&with, &rows).is_empty(),
            "{declaration}: a row classifies it"
        );
    }

    // A function said to reach no project that comes to be handed one is
    // refuted, whatever its reason says.
    let handed = header.replace(
        "simlin_sim_get_ltm_mode(SimlinSim *sim,",
        "simlin_sim_get_ltm_mode(SimlinModel *model,",
    );
    assert_ne!(handed, header, "the fixture rewrites the declaration");
    assert_eq!(handed_a_project(&handed), ["simlin_sim_get_ltm_mode"]);
}

// ── a row's footprint is what the entry point does ─────────────────────

/// How a call's lock events show it took the database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbShape {
    Untaken,
    /// Taken with the contents not held.
    Alone,
    /// Taken under the contents, which were released before the database was
    /// used and before it was released.
    ContentsReleased,
    /// Used, or released, while the contents were held.
    UnderContents,
}

/// What a call's lock events show it took: a tool session, the contents, and
/// the database in which shape. A simulation's own state is no project lock,
/// and is left out.
fn observed(events: &[Event]) -> Result<(bool, bool, DbShape), String> {
    /// One holding of the database, from its acquisition to its release.
    enum Held {
        /// Taken under the contents, and so far neither used nor outlived by
        /// them: a reader's or a writer's, not yet told apart.
        UnderContentsSoFar,
        Shown(DbShape),
    }
    let (mut session, mut contents) = (false, false);
    let mut shapes: Vec<DbShape> = Vec::new();
    let (mut contents_held, mut held): (bool, Option<Held>) = (false, None);
    for &event in events {
        match event {
            Event::Acquire(Rank::Session) => session = true,
            Event::Acquire(Rank::Contents) => {
                contents = true;
                contents_held = true;
            }
            Event::Release(Rank::Contents) => {
                contents_held = false;
                // Released with the database held and unused: a reader.
                if matches!(held, Some(Held::UnderContentsSoFar)) {
                    held = Some(Held::Shown(DbShape::ContentsReleased));
                }
            }
            Event::Acquire(Rank::Database) => {
                held = Some(if contents_held {
                    Held::UnderContentsSoFar
                } else {
                    Held::Shown(DbShape::Alone)
                });
            }
            Event::UseDatabase => {
                if contents_held && held.is_some() {
                    held = Some(Held::Shown(DbShape::UnderContents));
                }
            }
            Event::Release(Rank::Database) => shapes.push(match held.take() {
                // Released with the contents still held.
                Some(Held::UnderContentsSoFar) => DbShape::UnderContents,
                Some(Held::Shown(shape)) => shape,
                None => return Err("released a database it did not take".to_string()),
            }),
            Event::Release(Rank::Session)
            | Event::Acquire(Rank::SimState)
            | Event::Release(Rank::SimState) => {}
        }
    }
    let shape = match shapes.split_first() {
        None => DbShape::Untaken,
        Some((first, rest)) if rest.iter().all(|shape| shape == first) => *first,
        Some(_) => {
            return Err(format!(
                "took the database in more than one way: {shapes:?}"
            ))
        }
    };
    Ok((session, contents, shape))
}

/// The row run on a project with a database, a copy that shares its
/// datamodel, and a warm hit index: what the entry point did with the locks,
/// and what it left.
struct Observation {
    /// What the row answered.
    answer: String,
    events: Vec<Event>,
    /// The revision before and after the entry point itself.
    revision: (u64, u64),
    /// Whether the hit index, warm before the entry point, was kept; `None`
    /// for a model with no view to index.
    kept_index: Option<bool>,
    shares_with_copy: bool,
    changed_the_copy: bool,
    changed_itself: bool,
    /// For a row that writes: what the hit test answers at every point,
    /// before and after, and what an index of the datamodel as it is left
    /// answers.
    hits: Option<(Vec<Hit>, Vec<Hit>, Vec<Hit>)>,
    /// What the database answers before and after, and what one built from
    /// the contents as they are left answers.
    database: Option<(String, String, String)>,
}

unsafe fn observe(entry: &EntryPoint) -> Observation {
    let proj = open(!entry.viewless);
    let copy = copy_of(proj);
    let model = main_model(proj);
    drop((*proj).lock_db());
    let writes = entry.writes != Writes::Nothing;
    if !entry.viewless {
        // Warms the index.
        hit_at(model, (0.0, 0.0));
        assert!(has_index(proj), "the hit test caches its index");
    }
    let hits_before = (writes && !entry.viewless).then(|| hits(model));
    let database_before = writes.then(|| database_answers(proj));
    let before = datamodel_of(proj);

    let events = RefCell::new(Vec::new());
    let revisions = RefCell::new((0, 0));
    let answer = entry.run_on(proj, &|call| {
        let at = revision(proj);
        let ((), traced) = lock_order::trace::of(&mut *call);
        *events.borrow_mut() = traced;
        *revisions.borrow_mut() = (at, revision(proj));
    });

    let observation = Observation {
        answer,
        events: events.into_inner(),
        revision: revisions.into_inner(),
        kept_index: (!entry.viewless).then(|| has_index(proj)),
        shares_with_copy: shares_datamodel(proj, copy),
        changed_the_copy: *datamodel_of(copy) != *before,
        changed_itself: *datamodel_of(proj) != *before,
        hits: hits_before.map(|before| (before, hits(model), fresh_hits(proj))),
        database: database_before
            .map(|before| (before, database_answers(proj), fresh_database_answers(proj))),
    };
    simlin_model_unref(model);
    simlin_project_unref(copy);
    simlin_project_unref(proj);
    observation
}

/// Every row's observation, in the table's order, made once for the rules
/// that read it: a row's run is the same whichever rule asks.
fn observations() -> &'static [Observation] {
    static OBSERVED: std::sync::OnceLock<Vec<Observation>> = std::sync::OnceLock::new();
    OBSERVED.get_or_init(|| ALL.iter().map(|entry| unsafe { observe(entry) }).collect())
}

/// The rules every row is held to, on a project whose database exists: the
/// locks it takes are the ones it declares, taken in the shape it declares;
/// it changes the contents exactly when it declares it does, and a change
/// advances the revision, drops the hit index, leaves a copy that shared the
/// datamodel as it was, and leaves a database that answers as one built from
/// the contents; a read keeps the revision, the index and the sharing.
#[test]
fn an_entry_point_takes_the_locks_and_makes_the_changes_its_row_declares() {
    let mut failures = Vec::new();
    for (entry, seen) in ALL.iter().zip(observations()) {
        let mut fail = |what: String| failures.push(format!("{entry:?}: {what}"));

        match observed(&seen.events) {
            Err(why) => fail(format!("{why}: {:?}", seen.events)),
            Ok(took) => {
                let declared = (
                    entry.locks.session,
                    entry.locks.contents,
                    entry.locks.db.shape(),
                );
                if took != declared {
                    fail(format!(
                        "took (session, contents, database) = {took:?}, and its row declares {declared:?}: {:?}",
                        seen.events
                    ));
                }
            }
        }

        let writes = entry.writes != Writes::Nothing;
        let (before, after) = seen.revision;
        if writes != (after > before) {
            fail(format!(
                "revision {before} -> {after}, and its row declares {:?}",
                entry.writes
            ));
        }
        if writes != seen.changed_itself {
            fail(format!(
                "changed the contents: {}, and its row declares {:?}",
                seen.changed_itself, entry.writes
            ));
        }
        if seen.changed_the_copy {
            fail("changed a copy that shared the datamodel".to_string());
        }
        if writes == seen.shares_with_copy {
            fail(format!(
                "shares the datamodel with its copy: {}, and its row declares {:?}",
                seen.shares_with_copy, entry.writes
            ));
        }
        if let Some(kept_index) = seen.kept_index {
            if writes == kept_index {
                fail(format!(
                    "kept the hit index: {kept_index}, and its row declares {:?}",
                    entry.writes
                ));
            }
        }
        if let Some((before, after, fresh)) = &seen.hits {
            if after != fresh {
                fail(
                    "the hit test answers what no index of the datamodel it left answers"
                        .to_string(),
                );
            }
            // What makes the row able to catch a stale index at all.
            if entry.writes.redraws_main() == (fresh == before) {
                fail(format!(
                    "redrew the diagram: {}, and its row declares {:?}",
                    fresh != before,
                    entry.writes
                ));
            }
        }
        if let Some((before, after, fresh)) = &seen.database {
            if after != fresh {
                fail(format!(
                    "its database answers\n{after}where one built from its contents answers\n{fresh}"
                ));
            }
            // What makes the row able to catch a stale database at all.
            if (entry.writes == Writes::Model) == (fresh == before) {
                fail(format!(
                    "changed what the model simulates: {}, and its row declares {:?}",
                    fresh != before,
                    entry.writes
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The rows whose entry point never meets a project without a database: it
/// is handed a simulation, which `simlin_sim_new` compiled, edits on a
/// session that has read the model, as an edit requires, or reads a run the
/// session made.
const PREPARED_WITH_A_DATABASE: [(&str, &str); 5] = [
    ("simlin_analyze_get_loops_runtime", ""),
    ("simlin_analyze_get_links", ""),
    ("simlin_tool_session_call", "a tool that edits"),
    ("simlin_tool_session_call", "an edit its gate refuses"),
    ("simlin_tool_session_get_run", "a run it keeps"),
];

/// A project builds its database the first time an entry point that queries
/// it runs, and never before: an entry point that takes the database builds
/// one on a project with none and answers as on a project that has one, and
/// every other entry point builds nothing. A row that waits on a lock it
/// holds is refused by the lock order rather than left waiting.
#[test]
fn an_entry_point_builds_a_database_exactly_when_it_queries_one() {
    let mut failures = Vec::new();
    for (entry, built) in ALL.iter().zip(observations()) {
        unsafe {
            // What a project whose database existed before the row ran
            // answered.
            let expected = entry.locks.db.builds().then_some(&built.answer);

            let fresh = open(!entry.viewless);
            if (*fresh).has_db() {
                failures.push(format!("{entry:?}: opening built a database"));
            }
            let built_at_entry = std::cell::Cell::new(false);
            let answer = entry.run_on(fresh, &|call| {
                built_at_entry.set((*fresh).has_db());
                call();
            });
            if built_at_entry.get() != PREPARED_WITH_A_DATABASE.contains(&(entry.name, entry.case))
            {
                failures.push(format!(
                    "{entry:?}: its runner built the database before the entry point: {}",
                    built_at_entry.get()
                ));
            }
            if !built_at_entry.get() && (*fresh).has_db() != entry.locks.db.builds() {
                failures.push(format!(
                    "{entry:?}: built a database: {}, and its row declares {:?}",
                    (*fresh).has_db(),
                    entry.locks.db
                ));
            }
            match expected {
                Some(expected) if &answer != expected => failures.push(format!(
                    "{entry:?} answered {answer} on a project with no database, {expected} on one built at open"
                )),
                _ => {}
            }
            simlin_project_unref(fresh);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A copy shares the datamodel until either side is edited: the rows that
/// change the contents, run here on the COPY (the table's other rule runs
/// them on the original), leave the original exactly as it was, at the
/// revision it had, and the two no longer sharing.
#[test]
fn an_edit_of_a_copy_leaves_the_original_as_it_was() {
    let mut failures = Vec::new();
    for entry in ALL.iter().filter(|entry| entry.writes != Writes::Nothing) {
        unsafe {
            let original = open(!entry.viewless);
            let copy = copy_of(original);
            let before = datamodel_of(original);
            let (original_revision, copy_revision) = (revision(original), revision(copy));
            entry.run_plainly(copy);
            if revision(copy) <= copy_revision {
                failures.push(format!("{entry:?} of the copy left its revision"));
            }
            if revision(original) != original_revision {
                failures.push(format!(
                    "{entry:?} of the copy advanced the original's revision"
                ));
            }
            if *datamodel_of(original) != *before {
                failures.push(format!("{entry:?} of the copy reached the original"));
            }
            if *datamodel_of(copy) == *before {
                failures.push(format!("{entry:?} of the copy changed nothing"));
            }
            if shares_datamodel(original, copy) {
                failures.push(format!("{entry:?} of the copy left both sides sharing"));
            }
            simlin_project_unref(copy);
            simlin_project_unref(original);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every entry point of the person's that takes the database counts itself
/// while it waits for it, so a tool call that holds it stops: each row, run
/// against a database another thread holds, is seen waiting. A tool
/// session's own entry points wait uncounted (two calls that stopped for
/// each other would never finish), which `tests_concurrency.rs` pins.
#[test]
fn an_entry_point_that_waits_for_the_database_is_counted_while_it_waits() {
    let mut failures = Vec::new();
    for entry in ALL.iter().filter(|entry| entry.locks.db.counted()) {
        // A row that fails spends the whole positive wait, so the first one
        // ends the test rather than each later row spending it again.
        if !failures.is_empty() {
            break;
        }
        unsafe {
            let proj = open(!entry.viewless);
            drop((*proj).lock_db());
            let proj_addr = proj as usize;
            let (at_entry_tx, at_entry_rx) = mpsc::channel::<()>();
            let (held_tx, held_rx) = mpsc::channel::<()>();
            let waiter = thread::spawn(move || {
                let proj = proj_addr as *mut SimlinProject;
                entry.run_on(proj, &|call| {
                    // The database is taken by the test only now, so the
                    // runner's preparation is not what waits.
                    let _ = at_entry_tx.send(());
                    let _ = held_rx.recv_timeout(POSITIVE_WAIT);
                    call();
                });
            });
            let reached = at_entry_rx.recv_timeout(POSITIVE_WAIT).is_ok();
            let held = (*proj).lock_db();
            let _ = held_tx.send(());
            let deadline = Instant::now() + POSITIVE_WAIT;
            while reached && !(*proj).is_waited_on() && Instant::now() < deadline {
                thread::yield_now();
            }
            if !(*proj).is_waited_on() {
                failures.push(format!("{entry:?} waits for the database uncounted"));
            }
            drop(held);
            waiter.join().expect("the entry point returns");
            if (*proj).is_waited_on() {
                failures.push(format!("{entry:?} is counted after it has the database"));
            }
            simlin_project_unref(proj);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// An entry point that reads the contents and the database releases the
/// datamodel before it works: with each such row held inside its database
/// section, a revision read and a hit test from another thread answer, as
/// hover must while a simulation compiles.
#[test]
fn a_reader_of_the_database_leaves_the_datamodel_to_others_while_it_works() {
    let mut failures = Vec::new();
    for entry in ALL.iter().filter(|entry| entry.locks.db == Db::Reader) {
        // A row that fails spends the whole positive wait, so the first one
        // ends the test rather than each later row spending it again.
        if !failures.is_empty() {
            break;
        }
        unsafe {
            let proj = open(!entry.viewless);
            let proj_addr = proj as usize;
            let (entered_tx, entered_rx) = mpsc::channel::<()>();
            let release = Arc::new(AtomicBool::new(false));
            let armed = Arc::new(AtomicBool::new(false));
            let (release_in_hook, armed_in_hook) = (Arc::clone(&release), Arc::clone(&armed));
            let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
                if project as *const SimlinProject as usize == proj_addr
                    && armed_in_hook.swap(false, Ordering::SeqCst)
                {
                    let _ = entered_tx.send(());
                    let deadline = Instant::now() + POSITIVE_WAIT;
                    while !release_in_hook.load(Ordering::Acquire) && Instant::now() < deadline {
                        thread::yield_now();
                    }
                }
            }));
            let armed_at_entry = Arc::clone(&armed);
            let reader = thread::spawn(move || {
                let proj = proj_addr as *mut SimlinProject;
                entry.run_on(proj, &|call| {
                    armed_at_entry.store(true, Ordering::SeqCst);
                    call();
                });
            });
            if entered_rx.recv_timeout(POSITIVE_WAIT).is_err() {
                failures.push(format!("{entry:?} never reached its database section"));
            } else {
                let (answered_tx, answered_rx) = mpsc::channel::<()>();
                let other = thread::spawn(move || {
                    let proj = proj_addr as *mut SimlinProject;
                    revision(proj);
                    let model = main_model(proj);
                    let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
                    let mut err = ptr::null_mut();
                    // A model with no view reports that; what matters is that
                    // the hit test returns.
                    simlin_model_hit_test(
                        model, 0.0, 0.0, 1.0, &mut hit, &mut uid, &mut part, &mut err,
                    );
                    take_error(err);
                    simlin_model_unref(model);
                    let _ = answered_tx.send(());
                });
                if answered_rx.recv_timeout(POSITIVE_WAIT).is_err() {
                    failures.push(format!(
                        "{entry:?} kept a revision read and a hit test waiting while it worked"
                    ));
                }
                release.store(true, Ordering::Release);
                other.join().expect("the other thread returns");
            }
            release.store(true, Ordering::Release);
            reader.join().expect("the entry point returns");
            simlin_project_unref(proj);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
