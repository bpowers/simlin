// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::CString;
use std::ptr;

use simlin::*;

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

// ── PNG rendering FFI tests ─────────────────────────────────────────

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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
