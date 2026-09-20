// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a native host's working set costs, measured at the libsimlin FFI.
//!
//! A counting global allocator (over the system allocator, which a host runs
//! on when it builds libsimlin without the `mimalloc` feature) reports, after
//! each step a host takes, the heap bytes left live and the peak reached
//! during the step. Retained sizes are attributed by dropping one owner at a
//! time and reading how much the live count falls. The live count is the
//! heap alone: run one workload per process to read the process's own peak
//! from the operating system beside it. Results belong in the PR or chat, not
//! in a committed file.
//!
//! Workloads (`--workload`, default `all`):
//!
//! - `open`: open the model, which builds no database; the datamodel, and
//!   what a database synced to it holds, measured apart.
//! - `simulate`: compile and run, then read the series a diagram's sparklines
//!   draw, as a host does after opening.
//! - `diagnostics`: `simlin_project_get_errors` after the first run.
//! - `loops`: structural loops, a Loops That Matter run with its links, and
//!   loop discovery.
//! - `undo`: `--edits N` (default 50) equation edits landed the way a host
//!   lands them -- read the variable, upsert it, copy the project for undo,
//!   render the scene, simulate, fetch diagnostics -- keeping every copy, as an
//!   undo history does; then undo them all.
//! - `draft`: an equation draft previewed on a scratch copy of the project
//!   (upsert, diagnostics, a run), the way an equation editor previews one.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run --release -p simlin --example memory_census -- "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::{json, Value};
use simlin::*;

// --- counting allocator ------------------------------------------------------

// The `mimalloc` feature installs libsimlin's own global allocator, and a
// process has one, so the census counts only in a build without the feature
// (`main` says so and stops in one with it).
#[cfg_attr(feature = "mimalloc", allow(dead_code))]
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

#[cfg_attr(feature = "mimalloc", allow(dead_code))]
fn grow(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            grow(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            grow(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            if new_size >= layout.size() {
                grow(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[cfg(not(feature = "mimalloc"))]
#[global_allocator]
static GLOBAL: Counting = Counting;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn signed_mib(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Runs `f` as one step: prints the live heap after it, how much the step
/// changed it, the peak during it and its wall time.
fn step<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let before = live();
    PEAK.store(before, Ordering::Relaxed);
    let allocs = ALLOCS.load(Ordering::Relaxed);
    let start = Instant::now();
    let value = f();
    let elapsed = start.elapsed();
    let after = live();
    println!(
        "{name:<34} live {:>9.2} MiB  change {:>+9.2}  peak {:>9.2}  allocs {:>9}  {:>8.1} ms",
        mib(after),
        signed_mib(after as i64 - before as i64),
        mib(PEAK.load(Ordering::Relaxed)),
        ALLOCS.load(Ordering::Relaxed) - allocs,
        elapsed.as_secs_f64() * 1e3
    );
    value
}

/// Prints how much dropping one owner frees.
fn held_by(name: &str, drop_it: impl FnOnce()) {
    let before = live();
    drop_it();
    let freed = before as i64 - live() as i64;
    println!("  held by {name:<30} {:>9.2} MiB", signed_mib(freed));
}

// --- FFI helpers -------------------------------------------------------------

/// Panics with the error's message when `err` is set, freeing it.
unsafe fn check(err: *mut SimlinError, what: &str) {
    if err.is_null() {
        return;
    }
    let message = simlin_error_get_message(err);
    let message = if message.is_null() {
        String::new()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };
    simlin_error_free(err);
    panic!("{what}: {message}");
}

unsafe fn open_bytes(bytes: &[u8], extension: Option<&str>) -> *mut SimlinProject {
    let mut err = ptr::null_mut();
    let project = match extension {
        Some("mdl") => simlin_project_open_vensim(bytes.as_ptr(), bytes.len(), &mut err),
        Some("xmile" | "stmx" | "itmx") => {
            simlin_project_open_xmile(bytes.as_ptr(), bytes.len(), &mut err)
        }
        Some("json") => simlin_project_open_json(bytes.as_ptr(), bytes.len(), 0, &mut err),
        _ => panic!("not an .mdl, XMILE or JSON model"),
    };
    check(err, "opening the model");
    project
}

/// A copy of `project`, made the way a host copies one for its undo history:
/// a new project whose contents are replaced with `project`'s.
unsafe fn duplicate(project: *mut SimlinProject) -> *mut SimlinProject {
    let mut err = ptr::null_mut();
    let copy = simlin_project_new(ptr::null(), &mut err);
    check(err, "creating a project");
    simlin_project_replace_contents(copy, project, &mut err);
    check(err, "copying the project");
    copy
}

unsafe fn take_bytes(buf: *mut u8, len: usize) -> Vec<u8> {
    let bytes = std::slice::from_raw_parts(buf, len).to_vec();
    simlin_free(buf);
    bytes
}

unsafe fn take_json(buf: *mut u8, len: usize) -> Value {
    serde_json::from_slice(&take_bytes(buf, len)).expect("JSON")
}

unsafe fn render_scene(project: *mut SimlinProject, model_name: &CStr) -> Vec<u8> {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_project_render_scene(project, model_name.as_ptr(), &mut buf, &mut len, &mut err);
    check(err, "rendering the scene");
    take_bytes(buf, len)
}

unsafe fn names(
    count: usize,
    fill: impl FnOnce(*mut *mut std::os::raw::c_char, usize, &mut usize),
) -> Vec<String> {
    let mut raw = vec![ptr::null_mut(); count];
    let mut written = 0;
    fill(raw.as_mut_ptr(), count, &mut written);
    raw.into_iter()
        .take(written)
        .map(|p| {
            let name = CStr::from_ptr(p).to_string_lossy().into_owned();
            simlin_free_string(p);
            name
        })
        .collect()
}

/// A completed run and the series a host copied out of it.
struct Run {
    sim: *mut SimlinSim,
    series: Vec<Vec<f64>>,
}

impl Drop for Run {
    fn drop(&mut self) {
        unsafe { simlin_sim_unref(self.sim) };
    }
}

unsafe fn series(sim: *mut SimlinSim, name: &str, steps: usize) -> Vec<f64> {
    let c_name = CString::new(name).unwrap();
    let mut values = vec![f64::NAN; steps];
    let (mut written, mut err) = (0, ptr::null_mut());
    simlin_sim_get_series(
        sim,
        c_name.as_ptr(),
        values.as_mut_ptr(),
        steps,
        &mut written,
        &mut err,
    );
    check(err, "reading a series");
    values.truncate(written);
    values
}

/// Compiles and runs the model, then reads `time` and every series whose
/// variable a sparkline draws, as a host does after an edit.
unsafe fn simulate(model: *mut SimlinModel, ltm: bool, sparklines: &[String]) -> Run {
    let mut err = ptr::null_mut();
    let sim = simlin_sim_new(model, ltm, &mut err);
    check(err, "creating a simulation");
    simlin_sim_run_to_end(sim, &mut err);
    check(err, "running the simulation");
    let mut steps = 0;
    simlin_sim_get_stepcount(sim, &mut steps, &mut err);
    check(err, "reading the step count");
    let mut count = 0;
    simlin_sim_get_var_count(sim, &mut count, &mut err);
    check(err, "counting the variables");
    let all = names(count, |buf, max, written| {
        simlin_sim_get_var_names(sim, buf, max, written, &mut err)
    });
    check(err, "listing the variables");
    let wanted: std::collections::HashSet<&str> = sparklines.iter().map(String::as_str).collect();
    let mut read = vec![series(sim, "time", steps)];
    for name in &all {
        let base = match (name.ends_with(']'), name.find('[')) {
            (true, Some(bracket)) => &name[..bracket],
            _ => name.as_str(),
        };
        if name != "time" && wanted.contains(base) {
            read.push(series(sim, name, steps));
        }
    }
    Run { sim, series: read }
}

unsafe fn diagnostics(project: *mut SimlinProject) -> usize {
    let mut err = ptr::null_mut();
    let errors = simlin_project_get_errors(project, &mut err);
    check(err, "fetching diagnostics");
    if errors.is_null() {
        return 0;
    }
    let count = simlin_error_get_detail_count(errors);
    simlin_error_free(errors);
    count
}

/// Applies `patch` with errors allowed, as an editor does.
unsafe fn apply(project: *mut SimlinProject, patch: &[u8]) {
    let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
    simlin_project_apply_patch(
        project,
        patch.as_ptr(),
        patch.len(),
        false,
        true,
        &mut collected,
        &mut err,
    );
    if !collected.is_null() {
        simlin_error_free(collected);
    }
    check(err, "applying a patch");
}

/// The variable's record as JSON, which a host reads before it upserts the
/// variable with one field changed.
unsafe fn variable_json(model: *mut SimlinModel, ident: &str) -> Value {
    let c_ident = CString::new(ident).unwrap();
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_model_get_var_json(model, c_ident.as_ptr(), &mut buf, &mut len, &mut err);
    check(err, "reading a variable");
    take_json(buf, len)
}

unsafe fn sim_specs(model: *mut SimlinModel) -> Value {
    let (mut buf, mut len, mut err) = (ptr::null_mut(), 0, ptr::null_mut());
    simlin_model_get_sim_specs_json(model, &mut buf, &mut len, &mut err);
    check(err, "reading the simulation specs");
    take_json(buf, len)
}

/// The patch setting a constant aux's equation.
fn equation_patch(model_name: &str, record: &Value, equation: String) -> Vec<u8> {
    let mut aux = record.clone();
    aux["equation"] = json!(equation);
    serde_json::to_vec(&json!({
        "models": [{"name": model_name, "ops": [{"type": "upsertAux", "payload": {"aux": aux}}]}]
    }))
    .unwrap()
}

// --- the workloads -----------------------------------------------------------

struct Options {
    path: String,
    workload: String,
    edits: usize,
}

fn options() -> Options {
    let mut args = std::env::args().skip(1);
    let (mut path, mut workload, mut edits) = (None, "all".to_string(), 50);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workload" => workload = args.next().expect("--workload takes a name"),
            "--edits" => {
                edits = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--edits takes a count")
            }
            _ if path.is_none() => path = Some(arg),
            _ => panic!("unexpected argument '{arg}'"),
        }
    }
    Options {
        path: path.expect("usage: memory_census <model file> [--workload W] [--edits N]"),
        workload,
        edits,
    }
}

/// An open project with what a host reads from it once: its default model,
/// the model's name, the idents its sparklines draw and a constant to edit.
struct Session {
    project: *mut SimlinProject,
    model: *mut SimlinModel,
    model_name: String,
    c_name: CString,
    sparklines: Vec<String>,
    constant: Option<String>,
}

unsafe fn open_session(options: &Options) -> Session {
    let path = Path::new(&options.path);
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let project = step("open", || open_bytes(&bytes, extension.as_deref()));
    drop(bytes);
    let mut err = ptr::null_mut();
    let model = simlin_project_get_model(project, ptr::null(), &mut err);
    check(err, "getting the default model");
    let model_name = (*model).model_name.as_str().to_string();
    let c_name = CString::new(model_name.clone()).unwrap();

    let scene: Value = serde_json::from_slice(&render_scene(project, &c_name)).expect("scene");
    let mut sparklines: Vec<String> = scene["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| !e["sparkline"].is_null())
        .filter_map(|e| e["ident"].as_str().map(str::to_string))
        .collect();
    sparklines.sort();
    sparklines.dedup();

    let (mut buf, mut len) = (ptr::null_mut(), 0);
    simlin_project_serialize_json(project, 0, false, &mut buf, &mut len, &mut err);
    check(err, "serializing the project");
    let json = take_json(buf, len);
    let constant = json["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            let name = m["name"].as_str().unwrap_or_default();
            name == model_name || (model_name == "main" && name.is_empty())
        })
        .flat_map(|m| m["auxiliaries"].as_array().into_iter().flatten())
        .find(|aux| {
            aux["equation"]
                .as_str()
                .is_some_and(|e| e.trim().parse::<f64>().is_ok())
        })
        .and_then(|aux| aux["name"].as_str().map(str::to_string));
    Session {
        project,
        model,
        model_name,
        c_name,
        sparklines,
        constant,
    }
}

/// The datamodel and a database synced to it, measured apart.
unsafe fn open_breakdown(session: &Session) {
    let contents = (*session.project).datamodel.lock().unwrap();
    let project: &simlin_engine::datamodel::Project = &contents;
    let before = live();
    let copy = project.clone();
    let datamodel = live() - before;
    let variables: usize = {
        let before = live();
        let v: Vec<_> = copy.models.iter().map(|m| m.variables.clone()).collect();
        let size = live() - before;
        drop(v);
        size
    };
    let views: usize = {
        let before = live();
        let v: Vec<_> = copy.models.iter().map(|m| m.views.clone()).collect();
        let size = live() - before;
        drop(v);
        size
    };
    let before = live();
    let mut db = simlin_engine::db::SimlinDb::default();
    let empty_db = live() - before;
    db.sync(&copy);
    let synced_db = live() - before;
    println!(
        "  datamodel {:.2} MiB (variables {:.2}, views {:.2}); empty db {:.2} MiB, synced db {:.2} MiB",
        mib(datamodel),
        mib(variables),
        mib(views),
        mib(empty_db),
        mib(synced_db)
    );
    drop(db);
    drop(copy);
}

unsafe fn workload_open(options: &Options) {
    let session = open_session(options);
    open_breakdown(&session);
    close(session);
}

unsafe fn close(session: Session) {
    simlin_model_unref(session.model);
    simlin_project_unref(session.project);
}

unsafe fn workload_simulate(options: &Options) {
    let session = open_session(options);
    let run = step("simulate (compile, run, read)", || {
        simulate(session.model, false, &session.sparklines)
    });
    println!(
        "  {} sparkline idents, {} series read",
        session.sparklines.len(),
        run.series.len()
    );
    let again = step("simulate again (unchanged)", || {
        simulate(session.model, false, &session.sparklines)
    });
    held_by("the second run", || drop(again));
    let mut run = run;
    let series = std::mem::take(&mut run.series);
    held_by("the series read from the first", || drop(series));
    held_by("the first run's results", || drop(run));
    let before = live();
    close(session);
    println!(
        "  held by the project (compiled)     {:>9.2} MiB",
        mib(before - live())
    );
}

unsafe fn workload_diagnostics(options: &Options) {
    let session = open_session(options);
    let run = step("simulate", || {
        simulate(session.model, false, &session.sparklines)
    });
    let count = step("get_errors", || diagnostics(session.project));
    println!("  {count} diagnostics");
    step("get_errors again", || diagnostics(session.project));
    held_by("the run", || drop(run));
    let before = live();
    close(session);
    println!(
        "  held by the project (compiled)     {:>9.2} MiB",
        mib(before - live())
    );
}

unsafe fn workload_loops(options: &Options) {
    let session = open_session(options);
    let run = step("simulate", || {
        simulate(session.model, false, &session.sparklines)
    });
    step("get_errors", || diagnostics(session.project));
    let mut err = ptr::null_mut();
    let loops = step("structural loops", || {
        let loops = simlin_analyze_get_loops(session.model, &mut err);
        check(err, "structural loops");
        loops
    });
    println!("  {} loops", (*loops).count);
    held_by("the structural loops", || simlin_free_loops(loops));
    let ltm = step("LTM run (compile, run)", || {
        simulate(session.model, true, &[])
    });
    let links = step("LTM links", || {
        let links = simlin_analyze_get_links(ltm.sim, false, &mut err);
        check(err, "LTM links");
        links
    });
    held_by("the LTM links", || simlin_free_links(links));
    held_by("the LTM run", || drop(ltm));
    step("get_errors (LTM requested)", || {
        diagnostics(session.project)
    });
    let discovered = step("loop discovery (10 s budget)", || {
        let result = simlin_analyze_discover_loops(session.model, 10_000, &mut err);
        check(err, "loop discovery");
        result
    });
    held_by("the discovery result", || {
        simlin_free_discovery_result(discovered)
    });
    held_by("the plain run", || drop(run));
    let before = live();
    close(session);
    println!(
        "  held by the project (compiled)     {:>9.2} MiB",
        mib(before - live())
    );
}

unsafe fn workload_undo(options: &Options) {
    let session = open_session(options);
    let mut run = step("simulate", || {
        simulate(session.model, false, &session.sparklines)
    });
    step("get_errors", || diagnostics(session.project));
    let Some(ident) = session.constant.clone() else {
        println!("no constant aux to edit: undo skipped");
        close(session);
        return;
    };
    let mut heads = vec![step("first undo copy", || duplicate(session.project))];
    let edits_start = live();
    let mut scene = render_scene(session.project, &session.c_name);
    for i in 0..options.edits {
        let label = format!("edit {:>2}", i + 1);
        let quiet = i >= 3 && i + 1 < options.edits;
        let mut land = || {
            let record = variable_json(session.model, &ident);
            let value: f64 = record["equation"].as_str().unwrap().trim().parse().unwrap();
            let patch = equation_patch(&session.model_name, &record, format!("{}", value + 1.0));
            apply(session.project, &patch);
            sim_specs(session.model);
            heads.push(duplicate(session.project));
            let rendered = render_scene(session.project, &session.c_name);
            if rendered != scene {
                scene = rendered;
            }
            run = simulate(session.model, false, &session.sparklines);
            diagnostics(session.project);
        };
        if quiet {
            land();
        } else {
            step(&label, land);
        }
    }
    println!(
        "  {} edits: live {:.2} MiB above before the edits ({:.3} MiB per edit)",
        options.edits,
        signed_mib(live() as i64 - edits_start as i64),
        signed_mib(live() as i64 - edits_start as i64) / options.edits as f64
    );
    // Undo every edit: restore the copy from before it, then copy the result
    // for the session's head, as a host's undo does.
    let mut head: *mut SimlinProject = ptr::null_mut();
    let undo_start = live();
    for i in (0..options.edits).rev() {
        let mut restore = || {
            let mut err = ptr::null_mut();
            simlin_project_replace_contents(session.project, heads[i], &mut err);
            check(err, "restoring a copy");
            if !head.is_null() {
                simlin_project_unref(head);
            }
            head = duplicate(session.project);
            run = simulate(session.model, false, &session.sparklines);
            diagnostics(session.project);
        };
        if i == options.edits - 1 || i == 0 {
            step(&format!("undo to copy {i}"), restore);
        } else {
            restore();
        }
    }
    println!(
        "  {} undos: live {:+.2} MiB",
        options.edits,
        signed_mib(live() as i64 - undo_start as i64)
    );
    held_by("the head after undo", || simlin_project_unref(head));
    let count = heads.len();
    held_by(&format!("{count} undo copies"), || {
        for copy in heads.drain(..) {
            simlin_project_unref(copy);
        }
    });
    held_by("the run", || drop(run));
    let before = live();
    close(session);
    println!(
        "  held by the project (compiled)     {:>9.2} MiB",
        mib(before - live())
    );
}

unsafe fn workload_draft(options: &Options) {
    let session = open_session(options);
    let run = step("simulate", || {
        simulate(session.model, false, &session.sparklines)
    });
    step("get_errors", || diagnostics(session.project));
    let Some(ident) = session.constant.clone() else {
        println!("no constant aux to edit: draft skipped");
        close(session);
        return;
    };
    let scratch = step("scratch copy", || duplicate(session.project));
    let mut err = ptr::null_mut();
    let scratch_model = simlin_project_get_model(scratch, session.c_name.as_ptr(), &mut err);
    check(err, "getting the scratch model");
    let record = variable_json(session.model, &ident);
    let value: f64 = record["equation"].as_str().unwrap().trim().parse().unwrap();
    let mut preview = None;
    for i in 0..3 {
        step(&format!("preview draft {}", i + 1), || {
            let patch = equation_patch(
                &session.model_name,
                &record,
                format!("{}", value + i as f64),
            );
            apply(scratch, &patch);
            diagnostics(scratch);
            simlin_project_is_simulatable(scratch, session.c_name.as_ptr(), &mut err);
            check(err, "checking the draft");
            preview = Some(simulate(scratch_model, false, std::slice::from_ref(&ident)));
        });
    }
    held_by("the preview run", || drop(preview.take()));
    held_by("the scratch copy", || {
        simlin_model_unref(scratch_model);
        simlin_project_unref(scratch);
    });
    held_by("the run", || drop(run));
    let before = live();
    close(session);
    println!(
        "  held by the project (compiled)     {:>9.2} MiB",
        mib(before - live())
    );
}

fn main() {
    if cfg!(feature = "mimalloc") {
        eprintln!("memory_census counts through a global allocator of its own, which the `mimalloc` feature replaces: build it without the feature");
        std::process::exit(2);
    }
    let options = options();
    println!("{} ({} workload)", options.path, options.workload);
    unsafe {
        match options.workload.as_str() {
            "open" => workload_open(&options),
            "simulate" => workload_simulate(&options),
            "diagnostics" => workload_diagnostics(&options),
            "loops" => workload_loops(&options),
            "undo" => workload_undo(&options),
            "draft" => workload_draft(&options),
            "all" => {
                for workload in [
                    workload_open as unsafe fn(&Options),
                    workload_simulate,
                    workload_diagnostics,
                    workload_undo,
                    workload_draft,
                    workload_loops,
                ] {
                    workload(&options);
                    println!("  live after the workload: {:.2} MiB\n", mib(live()));
                }
            }
            other => panic!("unknown workload '{other}'"),
        }
    }
    println!("live at exit {:.2} MiB", mib(live()));
}
