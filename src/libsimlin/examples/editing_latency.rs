// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What diagram editing costs a native host, timed call by call at the
//! libsimlin FFI boundary.
//!
//! A host sends a pointer through `simlin_model_hit_test` whenever it moves
//! (hover feedback) and on every press, then plans the press with
//! `simlin_model_plan_tap` for a tap or `simlin_gesture_begin` for a drag. This
//! harness opens a model through the entry points a host calls and times each
//! call with `Instant`:
//!
//! - `hover`: hit tests at pseudo-random points over the diagram's content
//!   bounds, with the view unchanged between calls;
//! - `hover-near`: hit tests at points within the tolerance of every drawn
//!   element's position;
//! - `edit-first` and `edit-second`: the first and the second hit test after a
//!   view-only edit (an aux moved by one unit through
//!   `simlin_project_apply_patch`);
//! - `plan-tap` and `gesture-begin`: a press at every element's position,
//!   carrying the hit a host would resolve first;
//! - `plan-move`: every element but a link nudged alone by one unit, what each
//!   repeat of a held arrow key plans;
//! - `render-scene`: the scene a host redraws after every edit;
//! - `apply-equation` and `sim-new`: an equation edit, which validates through a
//!   compile, and the simulation a host creates after it. Both hold the
//!   datamodel lock throughout, so they are how long a press from another
//!   thread, or a hit test whose index an edit made stale, waits meanwhile;
//! - `hover-landing` and `hover-landing-warmed`: hit tests one display frame
//!   apart while another thread lands an equation edit every `LANDING_PERIOD`
//!   the way a host lands an edit (apply, then simulate, then fetch
//!   diagnostics, each holding the datamodel lock). A hit test with a current
//!   index answers without that lock, and the first after a commit rebuilds the
//!   index under it; `-warmed` hit-tests once on the landing thread right after
//!   each apply, so the rebuild happens before the simulation takes the lock.
//!
//! The harness keeps the system allocator, which a host runs on when it builds
//! libsimlin without the `mimalloc` feature. Results belong in the PR or chat,
//! not in a committed file.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run --release -p simlin --example editing_latency -- test/metasd/WRLD3-03/wrld3-03.mdl
//! ```
//!
//! Options: `--samples N` sets the calls per hover scenario (default 2000, and
//! a tenth of it for the edit scenarios), `--tolerance T` the hit tolerance in
//! model units (default 6, a pointer's slop at zoom 1).

use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use simlin::*;

/// Calls made before a scenario is timed, so its first samples are not the
/// cost of faulting in code and data.
const WARMUP: usize = 20;

/// Every element position a press lands at, capped so a large model keeps the
/// run short.
const MAX_PRESSES: usize = 1000;

/// Hit tests timed while edits land on another thread, one per display frame.
const HOVERS_WHILE_LANDING: usize = 600;

/// A display frame at 120 Hz, the rate a host sends hover hit tests at.
const FRAME: Duration = Duration::from_micros(8333);

/// How often the landing thread lands an edit: ten a second, a fast typist
/// landing every keystroke. Landing back to back would instead measure how long
/// a waiting hover is passed over by a thread that re-locks at once, not what a
/// landing costs a hover.
const LANDING_PERIOD: Duration = Duration::from_millis(100);

struct Options {
    path: String,
    samples: usize,
    tolerance: f64,
}

fn options() -> Options {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut samples = 2000;
    let mut tolerance = 6.0;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                samples = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--samples takes a count");
            }
            "--tolerance" => {
                tolerance = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--tolerance takes a number");
            }
            _ if path.is_none() => path = Some(arg),
            _ => panic!("unexpected argument '{arg}'"),
        }
    }
    Options {
        path: path.expect("usage: editing_latency <model file> [--samples N] [--tolerance T]"),
        samples,
        tolerance,
    }
}

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

unsafe fn open(path: &Path) -> *mut SimlinProject {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let mut err = ptr::null_mut();
    let project = match extension.as_deref() {
        Some("mdl") => simlin_project_open_vensim(bytes.as_ptr(), bytes.len(), &mut err),
        Some("xmile" | "stmx" | "itmx") => {
            simlin_project_open_xmile(bytes.as_ptr(), bytes.len(), &mut err)
        }
        Some("json") => simlin_project_open_json(bytes.as_ptr(), bytes.len(), 0, &mut err),
        _ => panic!("{}: not an .mdl, XMILE or JSON model", path.display()),
    };
    check(err, "opening the model");
    project
}

/// The JSON in a buffer libsimlin allocated, which this frees.
unsafe fn take_json(buf: *mut u8, len: usize) -> Value {
    let value = serde_json::from_slice(std::slice::from_raw_parts(buf, len)).expect("JSON");
    simlin_free(buf);
    value
}

/// A small deterministic generator (splitmix64), so every run samples the same
/// points.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

struct Model {
    project: *mut SimlinProject,
    model: *mut SimlinModel,
    tolerance: f64,
}

impl Model {
    unsafe fn hit(&self, x: f64, y: f64) -> Option<(i32, SimlinHitPart)> {
        let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
        let mut err = ptr::null_mut();
        simlin_model_hit_test(
            self.model,
            x,
            y,
            self.tolerance,
            &mut hit,
            &mut uid,
            &mut part,
            &mut err,
        );
        check(err, "a hit test");
        hit.then_some((uid, part))
    }
}

/// A scenario's samples, reported as a row.
fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let at = |q: f64| ms(samples[((samples.len() - 1) as f64 * q).round() as usize]);
    let mean = ms(samples.iter().sum::<Duration>()) / samples.len() as f64;
    println!(
        "{name:<14} {:>7} {:>10.4} {:>10.4} {:>10.4} {:>10.4} {:>10.4}",
        samples.len(),
        at(0.5),
        at(0.9),
        at(0.99),
        ms(*samples.last().unwrap()),
        mean
    );
}

fn timed<T>(samples: &mut Vec<Duration>, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let value = f();
    samples.push(start.elapsed());
    value
}

/// The patch upserting `aux` into `model_name`.
fn aux_patch(model_name: &str, aux: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "models": [{"name": model_name, "ops": [{"type": "upsertAux", "payload": {"aux": aux}}]}]
    }))
    .unwrap()
}

/// Applies `patch`, allowing errors as an editor does, and panics with the
/// error's message when it fails.
unsafe fn apply(project: *mut SimlinProject, patch: &[u8], what: &str) {
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
    check(err, what);
}

fn main() {
    let options = options();
    unsafe {
        let project = open(Path::new(&options.path));
        let mut err = ptr::null_mut();
        let model = simlin_project_get_model(project, ptr::null(), &mut err);
        check(err, "getting the default model");
        let model_name = (*model).model_name.as_str().to_string();
        let c_name = CString::new(model_name.clone()).unwrap();
        let m = Model {
            project,
            model,
            tolerance: options.tolerance,
        };

        let (mut buf, mut len) = (ptr::null_mut(), 0);
        simlin_project_render_scene(project, c_name.as_ptr(), &mut buf, &mut len, &mut err);
        check(err, "rendering the scene");
        let scene = take_json(buf, len);
        let drawn = scene["elements"].as_array().map_or(0, Vec::len);
        let bounds = &scene["contentBounds"];
        let (left, top, right, bottom) = (
            bounds["left"].as_f64().expect("content bounds"),
            bounds["top"].as_f64().unwrap(),
            bounds["right"].as_f64().unwrap(),
            bounds["bottom"].as_f64().unwrap(),
        );

        simlin_project_serialize_json(project, 0, false, &mut buf, &mut len, &mut err);
        check(err, "serializing the project");
        let json = take_json(buf, len);
        let view = json["models"]
            .as_array()
            .and_then(|models| {
                models.iter().find(|model| {
                    let name = model["name"].as_str().unwrap_or_default();
                    name == model_name || (model_name == "main" && name.is_empty())
                })
            })
            .map(|model| &model["views"][0]["elements"])
            .and_then(Value::as_array)
            .expect("the model's first view");
        // A press lands at an element's position; groups are containers whose
        // center belongs to what they hold, and links have no position.
        let positions: Vec<(f64, f64)> = view
            .iter()
            .filter(|e| !matches!(e["type"].as_str(), Some("group" | "link")))
            .filter_map(|e| Some((e["x"].as_f64()?, e["y"].as_f64()?)))
            .collect();
        let aux = view
            .iter()
            .find(|e| e["type"] == "aux")
            .cloned()
            .expect("an aux to move");

        println!(
            "{} (model '{model_name}'): {drawn} drawn elements, {} positioned, content {:.0}x{:.0}, tolerance {}",
            options.path,
            positions.len(),
            right - left,
            bottom - top,
            options.tolerance
        );
        println!(
            "{:<14} {:>7} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "scenario", "calls", "p50 ms", "p90 ms", "p99 ms", "max ms", "mean ms"
        );

        let mut rng = Rng(0x5eed);
        let margin = 4.0 * options.tolerance;
        let mut hover_point = move || {
            (
                rng.range(left - margin, right + margin),
                rng.range(top - margin, bottom + margin),
            )
        };

        let mut hover = Vec::with_capacity(options.samples);
        for i in 0..WARMUP + options.samples {
            let (x, y) = hover_point();
            if i < WARMUP {
                m.hit(x, y);
            } else {
                timed(&mut hover, || m.hit(x, y));
            }
        }
        report("hover", hover);

        let mut jitter = Rng(0x0b5e55ed);
        let mut near = Vec::with_capacity(options.samples);
        let mut hits = 0;
        for i in 0..WARMUP + options.samples {
            let (px, py) = positions[i % positions.len()];
            let t = options.tolerance;
            let (x, y) = (px + jitter.range(-t, t), py + jitter.range(-t, t));
            if i < WARMUP {
                m.hit(x, y);
            } else if timed(&mut near, || m.hit(x, y)).is_some() {
                hits += 1;
            }
        }
        report("hover-near", near);

        let edits = (options.samples / 10).max(1);
        let (mut first, mut second) = (Vec::new(), Vec::new());
        for i in 0..edits {
            // One unit to the side and back, alternately, so every edit moves it.
            let mut moved = aux.clone();
            let shift = if i % 2 == 0 { 1.0 } else { 0.0 };
            moved["x"] = json!(aux["x"].as_f64().unwrap() + shift);
            let patch = serde_json::to_vec(&json!({
                "models": [{
                    "name": model_name,
                    "ops": [{"type": "editView", "payload": {"index": 0, "upsert": [moved], "remove": []}}]
                }]
            }))
            .unwrap();
            let mut collected = ptr::null_mut();
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
            check(err, "applying a view-only edit");
            let (x, y) = hover_point();
            timed(&mut first, || m.hit(x, y));
            let (x, y) = hover_point();
            timed(&mut second, || m.hit(x, y));
        }
        report("edit-first", first);
        report("edit-second", second);

        let (mut taps, mut begins) = (Vec::new(), Vec::new());
        let presses = WARMUP + positions.len().min(MAX_PRESSES);
        for (i, &(x, y)) in positions.iter().cycle().take(presses).enumerate() {
            let hit = m.hit(x, y);
            let press = SimlinPress {
                x,
                y,
                has_hit: hit.is_some(),
                hit_uid: hit.map_or(0, |h| h.0),
                hit_part: hit.map_or(SimlinHitPart::Body, |h| h.1),
                tool: SimlinTool::None,
                selection: ptr::null(),
                selection_len: 0,
                toggle: false,
                pointer: SimlinPointerKind::Mouse,
                target_slop: options.tolerance,
            };
            let (mut discard_tap, mut discard_begin) = (Vec::new(), Vec::new());
            let (tap_samples, begin_samples) = if i < WARMUP {
                (&mut discard_tap, &mut discard_begin)
            } else {
                (&mut taps, &mut begins)
            };
            let (mut buf, mut len) = (ptr::null_mut(), 0);
            timed(tap_samples, || {
                simlin_model_plan_tap(model, &press, &mut buf, &mut len, &mut err)
            });
            check(err, "planning a tap");
            simlin_free(buf);
            let gesture = timed(begin_samples, || {
                simlin_gesture_begin(model, &press, &mut err)
            });
            check(err, "beginning a drag");
            simlin_gesture_unref(gesture);
        }
        println!("{hits} of the hover-near calls hit an element");
        report("plan-tap", taps);
        report("gesture-begin", begins);

        // A held arrow key: each element nudged alone, one unit per repeat.
        let movable: Vec<i32> = view
            .iter()
            .filter(|e| e["type"] != "link")
            .filter_map(|e| e["uid"].as_i64().and_then(|uid| i32::try_from(uid).ok()))
            .collect();
        let mut nudges = Vec::new();
        let repeats = WARMUP + movable.len().min(MAX_PRESSES);
        for (i, &uid) in movable.iter().cycle().take(repeats).enumerate() {
            let selection = [uid];
            let (mut buf, mut len) = (ptr::null_mut(), 0);
            let samples = if i < WARMUP {
                &mut Vec::new()
            } else {
                &mut nudges
            };
            timed(samples, || {
                simlin_model_plan_move(
                    model,
                    selection.as_ptr(),
                    1,
                    1.0,
                    0.0,
                    &mut buf,
                    &mut len,
                    &mut err,
                )
            });
            check(err, "planning a move");
            simlin_free(buf);
        }
        report("plan-move", nudges);

        // What a host redraws after every edit.
        let mut scenes = Vec::new();
        for i in 0..WARMUP + edits {
            let (mut buf, mut len) = (ptr::null_mut(), 0);
            let samples = if i < WARMUP {
                &mut Vec::new()
            } else {
                &mut scenes
            };
            timed(samples, || {
                simlin_project_render_scene(project, c_name.as_ptr(), &mut buf, &mut len, &mut err)
            });
            check(err, "rendering the scene");
            simlin_free(buf);
        }
        report("render-scene", scenes);

        // An equation edit applies through validation, which compiles, and the
        // host then simulates. Both hold the datamodel lock for their compile, so
        // these are how long a hit test or a press on another thread waits.
        let constant = json["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| {
                let name = model["name"].as_str().unwrap_or_default();
                name == model_name || (model_name == "main" && name.is_empty())
            })
            .flat_map(|model| model["auxiliaries"].as_array().into_iter().flatten())
            .find(|aux| {
                aux["equation"]
                    .as_str()
                    .is_some_and(|e| e.trim().parse::<f64>().is_ok())
            })
            .cloned();
        match constant {
            Some(constant) => {
                let value: f64 = constant["equation"]
                    .as_str()
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let (mut applies, mut sims) = (Vec::new(), Vec::new());
                for i in 0..edits.min(50) {
                    let mut edited = constant.clone();
                    edited["equation"] = json!(format!("{}", value + (i % 2 + 1) as f64));
                    let patch = aux_patch(&model_name, &edited);
                    timed(&mut applies, || {
                        apply(project, &patch, "applying an equation edit")
                    });
                    let sim = timed(&mut sims, || simlin_sim_new(model, false, &mut err));
                    check(err, "creating a simulation");
                    simlin_sim_unref(sim);
                }
                report("apply-equation", applies);
                report("sim-new", sims);

                // A host hovers while edits land off its UI thread.
                for (name, warmed) in [("hover-landing", false), ("hover-landing-warmed", true)] {
                    let stop = Arc::new(AtomicBool::new(false));
                    let lander = {
                        let stop = Arc::clone(&stop);
                        let (project, model) = (project as usize, model as usize);
                        let (constant, model_name) = (constant.clone(), model_name.clone());
                        thread::spawn(move || {
                            let (project, model) =
                                (project as *mut SimlinProject, model as *mut SimlinModel);
                            let mut i = 0;
                            while !stop.load(Ordering::Acquire) {
                                let landing = Instant::now();
                                let mut edited = constant.clone();
                                edited["equation"] =
                                    json!(format!("{}", value + (i % 2 + 1) as f64));
                                i += 1;
                                apply(
                                    project,
                                    &aux_patch(&model_name, &edited),
                                    "landing an equation edit",
                                );
                                let mut err = ptr::null_mut();
                                if warmed {
                                    let (mut hit, mut uid, mut part) =
                                        (false, 0, SimlinHitPart::Body);
                                    simlin_model_hit_test(
                                        model, 0.0, 0.0, 6.0, &mut hit, &mut uid, &mut part,
                                        &mut err,
                                    );
                                    check(err, "warming the hit index");
                                }
                                let sim = simlin_sim_new(model, false, &mut err);
                                check(err, "simulating a landed edit");
                                simlin_sim_unref(sim);
                                let errors = simlin_project_get_errors(project, &mut err);
                                check(err, "fetching a landed edit's diagnostics");
                                if !errors.is_null() {
                                    simlin_error_free(errors);
                                }
                                if let Some(rest) = LANDING_PERIOD.checked_sub(landing.elapsed()) {
                                    thread::sleep(rest);
                                }
                            }
                        })
                    };
                    let mut hovers = Vec::with_capacity(HOVERS_WHILE_LANDING);
                    for _ in 0..HOVERS_WHILE_LANDING {
                        thread::sleep(FRAME);
                        let (x, y) = hover_point();
                        timed(&mut hovers, || m.hit(x, y));
                    }
                    stop.store(true, Ordering::Release);
                    lander.join().expect("the landing thread lands every edit");
                    report(name, hovers);
                }
            }
            None => println!("no constant aux to edit: apply-equation and sim-new skipped"),
        }

        simlin_model_unref(m.model);
        simlin_project_unref(m.project);
    }
}
