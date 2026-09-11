// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::CString;
use std::ptr;

use simlin::*;
use simlin_engine::test_common::TestProject;

use crate::common::open_project_from_datamodel;

/// A small stock-and-flow datamodel with NO views, as produced by
/// building a model programmatically (e.g. through the patch API).
fn viewless_datamodel() -> simlin_engine::datamodel::Project {
    TestProject::new("viewless")
        .stock("population", "50", &["net_growth"], &[], None)
        .flow("net_growth", "population * rate", None)
        .aux("rate", "0.08", None)
        .build_datamodel()
}

#[test]
fn test_render_svg() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "project_open_xmile failed");
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("main").unwrap();
        simlin_project_render_svg(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "render_svg failed");
        assert!(!out_buffer.is_null());
        assert!(out_len > 0);

        let svg = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len)).unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("viewBox="));
        assert!(svg.contains("</svg>"));

        simlin_free(out_buffer);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg_generates_layout_for_viewless_model() {
    let datamodel = viewless_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("main").unwrap();
        simlin_project_render_svg(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        crate::common::expect_no_error(err, "render_svg on viewless model");
        assert!(!out_buffer.is_null());

        let svg = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len)).unwrap();
        assert!(svg.starts_with("<svg "));
        // Labels render display names (underscores become spaces, words may
        // wrap), so assert on single words.
        assert!(svg.contains("population"));
        assert!(svg.contains("growth"));
        simlin_free(out_buffer);

        // The generated layout is transient: rendering must not mutate
        // the project's persisted views.
        let mut ser_err: *mut SimlinError = ptr::null_mut();
        let mut ser_buf: *mut u8 = ptr::null_mut();
        let mut ser_len: usize = 0;
        simlin_project_serialize_protobuf(
            proj,
            &mut ser_buf as *mut *mut u8,
            &mut ser_len as *mut usize,
            &mut ser_err as *mut *mut SimlinError,
        );
        crate::common::expect_no_error(ser_err, "serialize after render");
        let roundtripped: simlin_engine::project_io::Project =
            prost::Message::decode(std::slice::from_raw_parts(ser_buf, ser_len)).unwrap();
        let deserialized = simlin_engine::serde::deserialize(roundtripped);
        let model = deserialized.get_model("main").unwrap();
        assert!(
            model.views.is_empty(),
            "render_svg must not persist a generated view"
        );
        simlin_free(ser_buf);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg_null_project() {
    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        let model_name = CString::new("main").unwrap();
        simlin_project_render_svg(
            ptr::null_mut(),
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);
        simlin_error_free(err);
    }
}

#[test]
fn test_render_svg_null_model_name() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        simlin_project_render_svg(
            proj,
            ptr::null(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg_nonexistent_model() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("nonexistent_model").unwrap();
        simlin_project_render_svg(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

// ── Scene rendering FFI tests ───────────────────────────────────────

/// Open the SIR fixture the SVG tests render.
unsafe fn open_sir() -> *mut SimlinProject {
    let data = std::fs::read("testdata/SIR.stmx").expect("missing SIR.stmx fixture");
    let mut err: *mut SimlinError = ptr::null_mut();
    let proj =
        simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
    crate::common::expect_no_error(err, "project_open_xmile");
    assert!(!proj.is_null());
    proj
}

/// Render `model_name`'s scene through the FFI and parse the returned JSON,
/// failing the test on any error.
unsafe fn render_scene_json(proj: *mut SimlinProject, model_name: &str) -> serde_json::Value {
    let mut err: *mut SimlinError = ptr::null_mut();
    let mut out_buffer: *mut u8 = ptr::null_mut();
    let mut out_len: usize = 0;
    let model_name = CString::new(model_name).unwrap();
    simlin_project_render_scene(
        proj,
        model_name.as_ptr(),
        &mut out_buffer as *mut *mut u8,
        &mut out_len as *mut usize,
        &mut err as *mut *mut SimlinError,
    );
    crate::common::expect_no_error(err, "render_scene");
    assert!(!out_buffer.is_null());
    let json: serde_json::Value =
        serde_json::from_slice(std::slice::from_raw_parts(out_buffer, out_len))
            .expect("the scene is JSON");
    simlin_free(out_buffer);
    json
}

/// Render `model_name`'s SVG through the FFI, failing the test on any error.
unsafe fn render_svg_text(proj: *mut SimlinProject, model_name: &str) -> String {
    let mut err: *mut SimlinError = ptr::null_mut();
    let mut out_buffer: *mut u8 = ptr::null_mut();
    let mut out_len: usize = 0;
    let model_name = CString::new(model_name).unwrap();
    simlin_project_render_svg(
        proj,
        model_name.as_ptr(),
        &mut out_buffer as *mut *mut u8,
        &mut out_len as *mut usize,
        &mut err as *mut *mut SimlinError,
    );
    crate::common::expect_no_error(err, "render_svg");
    let svg = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len))
        .unwrap()
        .to_string();
    simlin_free(out_buffer);
    svg
}

#[test]
fn test_render_scene() {
    unsafe {
        let proj = open_sir();
        let scene = render_scene_json(proj, "main");

        assert_eq!(scene["version"], 1);
        assert_eq!(scene["modelName"], "main");
        let elements = scene["elements"].as_array().expect("elements is an array");
        assert!(!elements.is_empty());
        for element in elements {
            assert!(
                !element["shapes"].as_array().unwrap().is_empty(),
                "a drawn element carries shapes: {element}"
            );
        }
        let kinds: Vec<&str> = elements
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        for kind in ["stock", "flow", "aux", "link"] {
            assert!(kinds.contains(&kind), "SIR draws a {kind}: {kinds:?}");
        }

        // `contentBounds` is the SVG viewBox before its padding, so the two
        // renderings of the same project agree on it.
        let svg = render_svg_text(proj, "main");
        let start = svg.find("viewBox=\"").unwrap() + "viewBox=\"".len();
        let end = start + svg[start..].find('"').unwrap();
        let view_box: Vec<i64> = svg[start..end]
            .split(' ')
            .map(|n| n.parse().unwrap())
            .collect();
        let bounds = &scene["contentBounds"];
        let left = bounds["left"].as_f64().unwrap().floor() as i64 - 10;
        let top = bounds["top"].as_f64().unwrap().floor() as i64 - 10;
        assert_eq!(view_box[0], left);
        assert_eq!(view_box[1], top);
        assert_eq!(
            view_box[2],
            (bounds["right"].as_f64().unwrap() - left as f64).ceil() as i64 + 10
        );
        assert_eq!(
            view_box[3],
            (bounds["bottom"].as_f64().unwrap() - top as f64).ceil() as i64 + 10
        );

        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_scene_generates_layout_for_viewless_model() {
    let datamodel = viewless_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let scene = render_scene_json(proj, "main");
        let idents: Vec<&str> = scene["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["ident"].as_str())
            .collect();
        for ident in ["population", "net_growth", "rate"] {
            assert!(
                idents.contains(&ident),
                "the generated layout draws {ident}: {idents:?}"
            );
        }

        // The generated layout is transient: rendering must not mutate the
        // project's persisted views.
        let mut ser_err: *mut SimlinError = ptr::null_mut();
        let mut ser_buf: *mut u8 = ptr::null_mut();
        let mut ser_len: usize = 0;
        simlin_project_serialize_protobuf(
            proj,
            &mut ser_buf as *mut *mut u8,
            &mut ser_len as *mut usize,
            &mut ser_err as *mut *mut SimlinError,
        );
        crate::common::expect_no_error(ser_err, "serialize after render");
        let roundtripped: simlin_engine::project_io::Project =
            prost::Message::decode(std::slice::from_raw_parts(ser_buf, ser_len)).unwrap();
        let deserialized = simlin_engine::serde::deserialize(roundtripped);
        assert!(
            deserialized.get_model("main").unwrap().views.is_empty(),
            "render_scene must not persist a generated view"
        );
        simlin_free(ser_buf);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_scene_null_project() {
    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        let model_name = CString::new("main").unwrap();
        simlin_project_render_scene(
            ptr::null_mut(),
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);
        simlin_error_free(err);
    }
}

#[test]
fn test_render_scene_null_output_pointers() {
    unsafe {
        let proj = open_sir();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model_name = CString::new("main").unwrap();
        simlin_project_render_scene(
            proj,
            model_name.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_scene_null_model_name() {
    unsafe {
        let proj = open_sir();
        let mut err: *mut SimlinError = ptr::null_mut();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        simlin_project_render_scene(
            proj,
            ptr::null(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);
        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_scene_nonexistent_model() {
    unsafe {
        let proj = open_sir();
        let mut err: *mut SimlinError = ptr::null_mut();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("nonexistent_model").unwrap();
        simlin_project_render_scene(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);
        let message = std::ffi::CStr::from_ptr(simlin_error_get_message(err))
            .to_string_lossy()
            .into_owned();
        assert!(
            message.contains("not found"),
            "the refusal names the missing model: {message}"
        );
        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

// ── PNG rendering FFI tests ─────────────────────────────────────────
//
// Gated on png_render (a default feature): `simlin_project_render_png`
// does not exist in a --no-default-features build (e.g. the browser wasm
// artifact), so these tests must compile out with it.

#[cfg(feature = "png_render")]
mod png {
    use super::*;

    /// PNG header magic bytes.
    const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

    #[test]
    fn test_render_png() {
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            panic!("missing SIR.stmx fixture");
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_xmile(
                data.as_ptr(),
                data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null(), "project_open_xmile failed");
            assert!(!proj.is_null());

            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let model_name = CString::new("main").unwrap();
            simlin_project_render_png(
                proj,
                model_name.as_ptr(),
                0,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null(), "render_png failed");
            assert!(!out_buffer.is_null());
            assert!(out_len > 8);

            let png_data = std::slice::from_raw_parts(out_buffer, out_len);
            assert_eq!(&png_data[0..8], &PNG_SIGNATURE, "missing PNG signature");

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_render_png_with_width() {
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            panic!("missing SIR.stmx fixture");
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_xmile(
                data.as_ptr(),
                data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(!proj.is_null());

            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let model_name = CString::new("main").unwrap();
            simlin_project_render_png(
                proj,
                model_name.as_ptr(),
                800,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null(), "render_png with width failed");
            assert!(!out_buffer.is_null());
            assert!(out_len > 8);

            let png_data = std::slice::from_raw_parts(out_buffer, out_len);
            assert_eq!(&png_data[0..8], &PNG_SIGNATURE);

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_render_png_generates_layout_for_viewless_model() {
        let datamodel = viewless_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let model_name = CString::new("main").unwrap();
            simlin_project_render_png(
                proj,
                model_name.as_ptr(),
                0,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            crate::common::expect_no_error(err, "render_png on viewless model");
            assert!(!out_buffer.is_null());
            assert!(out_len > 8);

            let png_data = std::slice::from_raw_parts(out_buffer, out_len);
            assert_eq!(&png_data[0..8], &PNG_SIGNATURE);

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_render_png_null_project() {
        unsafe {
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut err: *mut SimlinError = ptr::null_mut();
            let model_name = CString::new("main").unwrap();
            simlin_project_render_png(
                ptr::null_mut(),
                model_name.as_ptr(),
                0,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            assert!(out_buffer.is_null());
            assert_eq!(out_len, 0);
            simlin_error_free(err);
        }
    }

    #[test]
    fn test_render_png_null_model_name() {
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            panic!("missing SIR.stmx fixture");
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_xmile(
                data.as_ptr(),
                data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(!proj.is_null());

            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            simlin_project_render_png(
                proj,
                ptr::null(),
                0,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            assert!(out_buffer.is_null());
            assert_eq!(out_len, 0);

            simlin_error_free(err);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_render_png_nonexistent_model() {
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            panic!("missing SIR.stmx fixture");
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_xmile(
                data.as_ptr(),
                data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(!proj.is_null());

            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let model_name = CString::new("nonexistent_model").unwrap();
            simlin_project_render_png(
                proj,
                model_name.as_ptr(),
                0,
                0,
                &mut out_buffer as *mut *mut u8,
                &mut out_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            assert!(out_buffer.is_null());
            assert_eq!(out_len, 0);

            simlin_error_free(err);
            simlin_project_unref(proj);
        }
    }
}
