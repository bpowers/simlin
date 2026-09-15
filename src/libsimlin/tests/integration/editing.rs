// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The diagram editing FFI, from a press to an applied patch.
//!
//! What a tap or a drag plans is the engine's decision, pinned by the decision
//! tables in `simlin_engine::editing`'s tests. These tests pin the boundary:
//! each entry point marshals a press in and a plan out, the patch a plan carries
//! applies through `simlin_project_apply_patch` to the edit the plan described,
//! a model the editor cannot plan against is refused with a code a host can act
//! on, and a NULL input is an error rather than a crash.

use std::ffi::CString;
use std::ptr;

use serde_json::{json, Value};
use simlin::*;
use simlin_engine::datamodel::{self, Variable, ViewElement};
use simlin_engine::test_common::TestProject;

use crate::common::{expect_error_code, expect_no_error, open_project_from_datamodel};

const STOCK: i32 = 1;
const FLOW: i32 = 2;
const AUX: i32 = 4;
const LINK: i32 = 5;

/// A cloud filling a stock through a flow, an aux linked to the flow, and a
/// variable the diagram does not draw, loaded through the production JSON
/// conversion.
fn project() -> *mut SimlinProject {
    let project: simlin_engine::json::Project = serde_json::from_value(json!({
        "name": "editing",
        "simSpecs": {"startTime": 0.0, "endTime": 10.0, "dt": "1"},
        "models": [{
            "name": "main",
            "stocks": [{"name": "population", "initialEquation": "10", "inflows": ["births"], "outflows": []}],
            "flows": [{"name": "births", "equation": "population * rate"}],
            "auxiliaries": [{"name": "rate", "equation": "0.1"}, {"name": "undrawn", "equation": "1"}],
            "views": [{"elements": [
                {"type": "stock", "uid": STOCK, "name": "population", "x": 100.0, "y": 100.0},
                {"type": "flow", "uid": FLOW, "name": "births", "x": 38.75, "y": 100.0, "points": [
                    {"x": 0.0, "y": 100.0, "attachedToUid": 3},
                    {"x": 77.5, "y": 100.0, "attachedToUid": STOCK}
                ]},
                {"type": "cloud", "uid": 3, "flowUid": FLOW, "x": 0.0, "y": 100.0},
                {"type": "aux", "uid": AUX, "name": "rate", "x": 40.0, "y": 200.0},
                {"type": "link", "uid": LINK, "fromUid": AUX, "toUid": FLOW}
            ]}]
        }]
    }))
    .expect("a well-formed JSON project");
    open_project_from_datamodel(&datamodel::Project::from(project))
}

fn main_model(proj: *mut SimlinProject) -> *mut SimlinModel {
    let name = CString::new("main").unwrap();
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
        expect_no_error(err, "getting the model");
        assert!(!model.is_null());
        model
    }
}

/// A touch at `(x, y)`, with a finger's slop.
fn press(
    x: f64,
    y: f64,
    hit: Option<(i32, SimlinHitPart)>,
    tool: SimlinTool,
    selection: &[i32],
) -> SimlinPress {
    SimlinPress {
        x,
        y,
        has_hit: hit.is_some(),
        hit_uid: hit.map_or(0, |h| h.0),
        hit_part: hit.map_or(SimlinHitPart::Body, |h| h.1),
        tool,
        selection: if selection.is_empty() {
            ptr::null()
        } else {
            selection.as_ptr()
        },
        selection_len: selection.len(),
        toggle: false,
        pointer: SimlinPointerKind::Touch,
        target_slop: 10.0,
    }
}

/// The JSON in a buffer libsimlin allocated, which this frees.
unsafe fn take_json(buf: *mut u8, len: usize) -> Value {
    assert!(!buf.is_null(), "the call wrote no buffer");
    let value = serde_json::from_slice(std::slice::from_raw_parts(buf, len))
        .expect("the buffer holds JSON");
    simlin_free(buf);
    value
}

/// Applies a patch an editing entry point wrote, allowing errors as an editor
/// does: an element created mid-edit has an empty equation.
unsafe fn apply(proj: *mut SimlinProject, patch: &Value) {
    let bytes = serde_json::to_vec(patch).unwrap();
    let mut collected: *mut SimlinError = ptr::null_mut();
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_project_apply_patch(
        proj,
        bytes.as_ptr(),
        bytes.len(),
        false,
        true,
        &mut collected,
        &mut err,
    );
    if !collected.is_null() {
        simlin_error_free(collected);
    }
    expect_no_error(err, "applying the patch");
}

/// Reads main's variables and its first view.
fn inspect<T>(
    proj: *mut SimlinProject,
    read: impl FnOnce(&datamodel::Model, &[ViewElement]) -> T,
) -> T {
    let project = unsafe { (*proj).datamodel.lock().unwrap() };
    let model = project.get_model("main").expect("the model main");
    let Some(datamodel::View::StockFlow(view)) = model.views.first() else {
        panic!("main has no view");
    };
    read(model, &view.elements)
}

/// The uids an array names: its numbers, or its objects' `uid` fields.
fn uids(values: &Value) -> Vec<i64> {
    values
        .as_array()
        .expect("an array")
        .iter()
        .map(|v| v.as_i64().or_else(|| v["uid"].as_i64()).expect("a uid"))
        .collect()
}

/// The plan of moving `selection` by `(dx, dy)`.
unsafe fn plan_move(model: *mut SimlinModel, selection: &[i32], dx: f64, dy: f64) -> Value {
    let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
        (ptr::null_mut(), 0, ptr::null_mut());
    let uids = if selection.is_empty() {
        ptr::null()
    } else {
        selection.as_ptr()
    };
    simlin_model_plan_move(
        model,
        uids,
        selection.len(),
        dx,
        dy,
        &mut buf,
        &mut len,
        &mut err,
    );
    expect_no_error(err, "planning a move");
    take_json(buf, len)
}

#[test]
fn a_hit_lands_on_what_is_drawn_there() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Label);
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_model_hit_test(
            model, 100.0, 100.0, 10.0, &mut hit, &mut uid, &mut part, &mut err,
        );
        expect_no_error(err, "a hit test on the stock");
        assert!(hit, "the stock is drawn there");
        assert_eq!((uid, part), (STOCK, SimlinHitPart::Body));

        simlin_model_hit_test(
            model, 600.0, 600.0, 10.0, &mut hit, &mut uid, &mut part, &mut err,
        );
        expect_no_error(err, "a hit test on the empty canvas");
        assert!(!hit, "nothing is drawn there");

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_tap_with_a_creation_tool_creates_the_element_and_hands_off_its_name() {
    let proj = project();
    let model = main_model(proj);
    let before = inspect(proj, |model, _| model.variables.len());
    unsafe {
        let p = press(300.0, 300.0, None, SimlinTool::Aux, &[]);
        let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
            (ptr::null_mut(), 0, ptr::null_mut());
        simlin_model_plan_tap(model, &p, &mut buf, &mut len, &mut err);
        expect_no_error(err, "planning the tap");
        let plan = take_json(buf, len);
        assert_eq!(plan["commit"], "edit", "{plan}");
        let handoff = plan["handoff"]
            .as_i64()
            .expect("a created element hands off its name") as i32;
        apply(proj, &plan["patch"]);
        inspect(proj, |model, view| {
            let Some(ViewElement::Aux(aux)) = view.iter().find(|e| e.get_uid() == handoff) else {
                panic!("the handed-off element is the created aux");
            };
            assert!(
                model.get_variable(&aux.name).is_some(),
                "the created aux names a variable"
            );
            assert_eq!(model.variables.len(), before + 1);
        });
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_drag_previews_its_frames_and_commits_the_edit_they_showed() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let p = press(
            100.0,
            100.0,
            Some((STOCK, SimlinHitPart::Body)),
            SimlinTool::None,
            &[STOCK],
        );
        let mut err: *mut SimlinError = ptr::null_mut();
        let gesture = simlin_gesture_begin(model, &p, &mut err);
        expect_no_error(err, "beginning the drag");
        assert!(!gesture.is_null(), "a press on a stock starts a drag");

        let (mut frame_buf, mut frame_len): (*const u8, usize) = (ptr::null(), 0);
        simlin_gesture_frame(
            gesture,
            160.0,
            130.0,
            &mut frame_buf,
            &mut frame_len,
            &mut err,
        );
        expect_no_error(err, "planning a frame");
        let frame: Value = serde_json::from_slice(std::slice::from_raw_parts(frame_buf, frame_len))
            .expect("the frame is JSON");
        assert_eq!(frame["kind"], "moveSelection", "{frame}");
        assert_eq!(frame["commit"], "edit", "{frame}");
        assert!(
            uids(&frame["hidden"]).contains(&(STOCK as i64)),
            "the frame hides the stock it redraws: {frame}"
        );
        assert!(
            uids(&frame["elements"]).contains(&(STOCK as i64)),
            "and draws it where it moved: {frame}"
        );

        let (mut buf, mut len): (*mut u8, usize) = (ptr::null_mut(), 0);
        simlin_gesture_commit(gesture, 160.0, 130.0, &mut buf, &mut len, &mut err);
        expect_no_error(err, "committing the drag");
        simlin_gesture_unref(gesture);
        let plan = take_json(buf, len);
        assert_eq!(plan["commit"], "edit", "{plan}");
        apply(proj, &plan["patch"]);
        inspect(proj, |model, view| {
            let Some(ViewElement::Stock(stock)) = view.iter().find(|e| e.get_uid() == STOCK) else {
                panic!("the stock is still drawn");
            };
            assert_eq!((stock.x, stock.y), (160.0, 130.0));
            let Some(Variable::Stock(population)) = model.get_variable("population") else {
                panic!("population is a stock");
            };
            assert_eq!(
                population.inflows,
                ["births"],
                "a move changes geometry, not what fills the stock"
            );
        });
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_drag_onto_a_refused_target_marks_it_and_commits_nothing() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        // A second link from the aux to the flow it already links to.
        let p = press(
            40.0,
            200.0,
            Some((AUX, SimlinHitPart::Body)),
            SimlinTool::Link,
            &[],
        );
        let mut err: *mut SimlinError = ptr::null_mut();
        let gesture = simlin_gesture_begin(model, &p, &mut err);
        expect_no_error(err, "beginning the link");
        assert!(!gesture.is_null(), "the link tool draws from an aux");

        let (mut frame_buf, mut frame_len): (*const u8, usize) = (ptr::null(), 0);
        simlin_gesture_frame(
            gesture,
            38.75,
            100.0,
            &mut frame_buf,
            &mut frame_len,
            &mut err,
        );
        expect_no_error(err, "planning a frame");
        let frame: Value = serde_json::from_slice(std::slice::from_raw_parts(frame_buf, frame_len))
            .expect("the frame is JSON");
        assert_eq!(frame["kind"], "createLink", "{frame}");
        assert_eq!(
            frame["target"],
            json!({"uid": FLOW, "valid": false}),
            "{frame}"
        );

        let (mut buf, mut len): (*mut u8, usize) = (ptr::null_mut(), 0);
        simlin_gesture_commit(gesture, 38.75, 100.0, &mut buf, &mut len, &mut err);
        expect_no_error(err, "committing the link");
        simlin_gesture_unref(gesture);
        let plan = take_json(buf, len);
        assert_eq!(plan["commit"], "none", "{plan}");
        assert_eq!(
            plan["patch"],
            Value::Null,
            "a refused drop has no patch: {plan}"
        );
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_press_that_starts_no_drag_is_null_without_an_error() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        // The link tool draws from an element, and the empty canvas is none.
        let p = press(600.0, 600.0, None, SimlinTool::Link, &[]);
        let mut err: *mut SimlinError = ptr::null_mut();
        let gesture = simlin_gesture_begin(model, &p, &mut err);
        assert!(gesture.is_null());
        assert!(
            err.is_null(),
            "a press that starts no drag is not a failure"
        );
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_drag_back_to_its_press_commits_none_without_a_patch() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let p = press(
            100.0,
            100.0,
            Some((STOCK, SimlinHitPart::Body)),
            SimlinTool::None,
            &[STOCK],
        );
        let mut err: *mut SimlinError = ptr::null_mut();
        let gesture = simlin_gesture_begin(model, &p, &mut err);
        expect_no_error(err, "beginning the drag");
        assert!(!gesture.is_null(), "a press on a stock starts a drag");
        let (mut buf, mut len): (*mut u8, usize) = (ptr::null_mut(), 0);
        simlin_gesture_commit(gesture, 100.0, 100.0, &mut buf, &mut len, &mut err);
        expect_no_error(err, "committing the drag");
        simlin_gesture_unref(gesture);
        let plan = take_json(buf, len);
        assert_eq!(plan["commit"], "none", "{plan}");
        assert_eq!(plan["patch"], Value::Null, "{plan}");
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_planned_move_moves_the_selection_and_its_flow_follows() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let plan = plan_move(model, &[STOCK], 10.0, 20.0);
        assert_eq!(plan["kind"], "moveSelection", "{plan}");
        assert_eq!(plan["commit"], "edit", "{plan}");
        assert_eq!(plan["label"], "move", "{plan}");
        assert_eq!(uids(&plan["selection"]), [STOCK as i64], "{plan}");
        apply(proj, &plan["patch"]);
        inspect(proj, |model, view| {
            let Some(ViewElement::Stock(stock)) = view.iter().find(|e| e.get_uid() == STOCK) else {
                panic!("the stock is still drawn");
            };
            assert_eq!((stock.x, stock.y), (110.0, 120.0));
            let Some(ViewElement::Flow(flow)) = view.iter().find(|e| e.get_uid() == FLOW) else {
                panic!("the flow is still drawn");
            };
            assert_eq!(
                flow.points.last().and_then(|p| p.attached_to_uid),
                Some(STOCK),
                "the flow still fills the stock it followed"
            );
            let Some(Variable::Stock(population)) = model.get_variable("population") else {
                panic!("population is a stock");
            };
            assert_eq!(population.inflows, ["births"]);
        });
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_move_that_lands_nothing_commits_none_without_a_patch() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let rows: [(&str, &[i32], f64, f64); 4] = [
            ("a lone link", &[LINK], 10.0, 0.0),
            ("an empty selection", &[], 10.0, 0.0),
            ("a zero offset", &[AUX], 0.0, 0.0),
            ("a non-finite offset", &[AUX], f64::NAN, 0.0),
        ];
        for (name, selection, dx, dy) in rows {
            let plan = plan_move(model, selection, dx, dy);
            assert_eq!(
                (&plan["commit"], &plan["label"], &plan["patch"]),
                (&json!("none"), &json!(""), &Value::Null),
                "{name}: {plan}"
            );
        }
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_planned_delete_removes_the_variable_and_every_link_touching_it() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let selection = [AUX];
        let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
            (ptr::null_mut(), 0, ptr::null_mut());
        simlin_model_plan_delete(
            model,
            selection.as_ptr(),
            selection.len(),
            &mut buf,
            &mut len,
            &mut err,
        );
        expect_no_error(err, "planning the delete");
        apply(proj, &take_json(buf, len));
        inspect(proj, |model, view| {
            assert!(
                model.get_variable("rate").is_none(),
                "the aux's variable is deleted"
            );
            let drawn: Vec<i32> = view.iter().map(ViewElement::get_uid).collect();
            assert!(
                !drawn.contains(&AUX) && !drawn.contains(&LINK),
                "the aux and its link are gone: {drawn:?}"
            );
        });
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_planned_rename_goes_through_the_diagram_when_the_variable_is_drawn() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let rename = |old: &str, new: &str| -> Value {
            let (old, new) = (CString::new(old).unwrap(), CString::new(new).unwrap());
            let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
                (ptr::null_mut(), 0, ptr::null_mut());
            simlin_model_plan_rename(
                model,
                old.as_ptr(),
                new.as_ptr(),
                &mut buf,
                &mut len,
                &mut err,
            );
            expect_no_error(err, "planning the rename");
            take_json(buf, len)
        };

        let drawn = rename("rate", "growth rate");
        assert_eq!(
            drawn["models"][0]["ops"][0]["type"], "editView",
            "a drawn variable is renamed by relabeling its element: {drawn}"
        );
        apply(proj, &drawn);

        let undrawn = rename("undrawn", "offstage");
        assert_eq!(
            undrawn["models"][0]["ops"][0]["type"], "renameVariable",
            "a variable with no element is renamed directly: {undrawn}"
        );
        apply(proj, &undrawn);

        inspect(proj, |model, view| {
            for (old, new) in [("rate", "growth_rate"), ("undrawn", "offstage")] {
                assert!(model.get_variable(old).is_none(), "{old} is renamed");
                assert!(model.get_variable(new).is_some(), "to {new}");
            }
            let Some(ViewElement::Aux(aux)) = view.iter().find(|e| e.get_uid() == AUX) else {
                panic!("the aux is still drawn");
            };
            assert_eq!(aux.name, "growth rate");
        });
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn a_model_with_no_diagram_is_refused_with_does_not_exist() {
    let datamodel = TestProject::new("viewless")
        .aux("rate", "0.1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let model = main_model(proj);
    unsafe {
        let p = press(0.0, 0.0, None, SimlinTool::Aux, &[]);
        let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
            (ptr::null_mut(), 0, ptr::null_mut());
        simlin_model_plan_tap(model, &p, &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::DoesNotExist, "a tap");
        assert!(buf.is_null());

        let gesture = simlin_gesture_begin(model, &p, &mut err);
        assert!(gesture.is_null());
        expect_error_code(err, SimlinErrorCode::DoesNotExist, "a drag");

        let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
        simlin_model_hit_test(
            model, 0.0, 0.0, 10.0, &mut hit, &mut uid, &mut part, &mut err,
        );
        expect_error_code(err, SimlinErrorCode::DoesNotExist, "a hit test");

        let selection = [1];
        simlin_model_plan_delete(model, selection.as_ptr(), 1, &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::DoesNotExist, "a delete");

        simlin_model_plan_move(
            model,
            selection.as_ptr(),
            1,
            1.0,
            0.0,
            &mut buf,
            &mut len,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::DoesNotExist, "a move");

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn null_inputs_are_errors_not_crashes() {
    let proj = project();
    let model = main_model(proj);
    unsafe {
        let (mut buf, mut len, mut err): (*mut u8, usize, *mut SimlinError) =
            (ptr::null_mut(), 0, ptr::null_mut());
        simlin_model_plan_tap(model, ptr::null(), &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL press");

        let mut unbacked = press(0.0, 0.0, None, SimlinTool::None, &[]);
        unbacked.selection_len = 3;
        simlin_model_plan_tap(model, &unbacked, &mut buf, &mut len, &mut err);
        expect_error_code(
            err,
            SimlinErrorCode::Generic,
            "a NULL selection with a length",
        );

        let gesture = simlin_gesture_begin(model, ptr::null(), &mut err);
        assert!(gesture.is_null());
        expect_error_code(err, SimlinErrorCode::Generic, "beginning with a NULL press");

        let (mut frame_buf, mut frame_len): (*const u8, usize) = (ptr::null(), 0);
        simlin_gesture_frame(
            ptr::null_mut(),
            0.0,
            0.0,
            &mut frame_buf,
            &mut frame_len,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "a frame of a NULL gesture");

        simlin_gesture_commit(ptr::null_mut(), 0.0, 0.0, &mut buf, &mut len, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "committing a NULL gesture");

        simlin_model_plan_delete(model, ptr::null(), 2, &mut buf, &mut len, &mut err);
        expect_error_code(
            err,
            SimlinErrorCode::Generic,
            "a NULL selection with a length",
        );

        simlin_model_plan_move(
            model,
            ptr::null(),
            2,
            1.0,
            0.0,
            &mut buf,
            &mut len,
            &mut err,
        );
        expect_error_code(
            err,
            SimlinErrorCode::Generic,
            "a NULL selection with a length to move",
        );

        let selection = [STOCK];
        simlin_model_plan_move(
            model,
            selection.as_ptr(),
            1,
            1.0,
            0.0,
            ptr::null_mut(),
            &mut len,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL output buffer");

        simlin_model_plan_rename(
            model,
            ptr::null(),
            ptr::null(),
            &mut buf,
            &mut len,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "a NULL name");

        simlin_gesture_ref(ptr::null_mut());
        simlin_gesture_unref(ptr::null_mut());
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
