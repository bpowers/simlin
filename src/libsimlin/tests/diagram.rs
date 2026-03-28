// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::CString;
use std::ptr;

use simlin::*;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

#[test]
fn test_diagram_sync_sir_model() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        eprintln!("missing SIR.stmx fixture; skipping");
        return;
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err);
        assert!(err.is_null());
        assert!(!proj.is_null());

        let model_name = CString::new("main").unwrap();
        err = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null(), "diagram_sync should succeed for SIR model");

        // Verify the model now has a view with elements
        let datamodel_locked = (*proj).datamodel.lock().unwrap();
        let model = datamodel_locked.get_model("main").unwrap();
        assert_eq!(model.views.len(), 1);
        match &model.views[0] {
            engine::datamodel::View::StockFlow(sf) => {
                assert!(
                    !sf.elements.is_empty(),
                    "layout should produce view elements"
                );
            }
        }
        drop(datamodel_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_test_project() {
    let test_project = TestProject::new("layout_test")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null(), "diagram_sync should succeed");

        let datamodel_locked = (*proj).datamodel.lock().unwrap();
        let model = datamodel_locked.get_model("main").unwrap();
        assert_eq!(model.views.len(), 1);
        match &model.views[0] {
            engine::datamodel::View::StockFlow(sf) => {
                assert!(
                    !sf.elements.is_empty(),
                    "layout should produce view elements"
                );
                assert!(
                    sf.view_box.width > 0.0 && sf.view_box.height > 0.0,
                    "view_box should be non-zero"
                );
            }
        }
        drop(datamodel_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_preserves_zoom() {
    let test_project = TestProject::new("zoom_test")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["inflow"], &[], None)
        .flow("inflow", "10", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // First, generate a layout
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null());

        // Manually set zoom to 2.0
        {
            let mut datamodel_locked = (*proj).datamodel.lock().unwrap();
            let model = datamodel_locked.get_model_mut("main").unwrap();
            if let Some(engine::datamodel::View::StockFlow(sf)) = model.views.first_mut() {
                sf.zoom = 2.0;
            }
        }

        // Generate layout again -- zoom should be preserved
        err = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null());

        let datamodel_locked = (*proj).datamodel.lock().unwrap();
        let model = datamodel_locked.get_model("main").unwrap();
        match &model.views[0] {
            engine::datamodel::View::StockFlow(sf) => {
                assert!(
                    (sf.zoom - 2.0).abs() < f64::EPSILON,
                    "zoom should be preserved, got {}",
                    sf.zoom,
                );
            }
        }
        drop(datamodel_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_idempotent() {
    let test_project = TestProject::new("idempotent_test")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("x", "10", &["dx"], &[], None)
        .flow("dx", "1", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();

        // Call twice
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null());
        err = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null());

        let datamodel_locked = (*proj).datamodel.lock().unwrap();
        let model = datamodel_locked.get_model("main").unwrap();
        assert_eq!(model.views.len(), 1);
        drop(datamodel_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_null_project() {
    unsafe {
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(ptr::null_mut(), model_name.as_ptr(), ptr::null(), &mut err);
        assert!(!err.is_null(), "null project should produce an error");
        simlin_error_free(err);
    }
}

#[test]
fn test_diagram_sync_null_model_name() {
    let test_project = TestProject::new("null_name")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("a", "1", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(proj, ptr::null(), &mut err);
        assert!(!err.is_null(), "null model name should produce an error");
        simlin_error_free(err);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_nonexistent_model() {
    let test_project = TestProject::new("nonexistent")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("a", "1", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("no_such_model").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(
            !err.is_null(),
            "nonexistent model name should produce an error"
        );
        simlin_error_free(err);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_diagram_sync_empty_model_name() {
    let test_project = TestProject::new("empty_name")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("a", "1", None);

    let datamodel = test_project.build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(!err.is_null(), "empty model name should produce an error");
        simlin_error_free(err);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_ac7_2_incremental_layout_via_patch_json() {
    let datamodel = TestProject::new("ac7_2_incr_layout")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();

        simlin_project_diagram_sync(proj, model_name.as_ptr(), ptr::null(), &mut err);
        assert!(err.is_null(), "initial diagram_sync should succeed");

        let initial_positions: Vec<(String, f64, f64)> = {
            let dm = (*proj).datamodel.lock().unwrap();
            let model = dm.get_model("main").unwrap();
            assert_eq!(model.views.len(), 1);
            match &model.views[0] {
                engine::datamodel::View::StockFlow(sf) => {
                    assert!(!sf.elements.is_empty());
                    sf.elements
                        .iter()
                        .filter_map(|e| {
                            let name = e.get_name()?.to_string();
                            let (x, y) = match e {
                                engine::datamodel::ViewElement::Aux(a) => (a.x, a.y),
                                engine::datamodel::ViewElement::Stock(s) => (s.x, s.y),
                                engine::datamodel::ViewElement::Flow(f) => (f.x, f.y),
                                _ => return None,
                            };
                            Some((name, x, y))
                        })
                        .collect()
                }
            }
        };

        assert!(initial_positions.len() >= 3);

        let patch_json = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "extra", "equation": "42" } }
                }]
            }]
        }"#;
        let patch_bytes = patch_json.as_bytes();
        let mut collected: *mut SimlinError = ptr::null_mut();
        err = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected,
            &mut err,
        );
        assert!(err.is_null(), "patch should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        let patch_cstr = CString::new(patch_json).unwrap();
        err = ptr::null_mut();
        simlin_project_diagram_sync(proj, model_name.as_ptr(), patch_cstr.as_ptr(), &mut err);
        assert!(err.is_null(), "incremental diagram_sync should succeed");

        {
            let dm = (*proj).datamodel.lock().unwrap();
            let model = dm.get_model("main").unwrap();
            assert_eq!(model.views.len(), 1);
            match &model.views[0] {
                engine::datamodel::View::StockFlow(sf) => {
                    let has_extra =
                        sf.elements.iter().any(|e| e.get_name().is_some_and(|n| n == "extra"));
                    assert!(has_extra, "newly added 'extra' must appear in the view");

                    for (name, orig_x, orig_y) in &initial_positions {
                        let found = sf
                            .elements
                            .iter()
                            .find(|e| e.get_name().is_some_and(|n| n == name));
                        let elem = found
                            .unwrap_or_else(|| panic!("element '{name}' must still be in the view"));
                        let (x, y) = match elem {
                            engine::datamodel::ViewElement::Aux(a) => (a.x, a.y),
                            engine::datamodel::ViewElement::Stock(s) => (s.x, s.y),
                            engine::datamodel::ViewElement::Flow(f) => (f.x, f.y),
                            _ => panic!("unexpected element type for '{name}'"),
                        };
                        assert!(
                            (x - orig_x).abs() < 1.0 && (y - orig_y).abs() < 1.0,
                            "'{name}' position should be preserved: \
                             expected ({orig_x},{orig_y}), got ({x},{y})"
                        );
                    }
                }
            }
        }

        simlin_project_unref(proj);
    }
}
