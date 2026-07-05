// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the standalone results FFI (`simlin_results_*`),
//! exercised through its only producer today: Vensim VDF import.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use simlin::{
    simlin_error_free, simlin_error_get_code, simlin_error_get_message, simlin_free_string,
    simlin_results_get_series, simlin_results_get_stepcount, simlin_results_get_var_count,
    simlin_results_get_var_names, simlin_results_open_vdf, simlin_results_ref,
    simlin_results_unref, SimlinError, SimlinErrorCode, SimlinResults,
};

fn open_vdf(bytes: &[u8]) -> Result<*mut SimlinResults, (SimlinErrorCode, String)> {
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let results = simlin_results_open_vdf(bytes.as_ptr(), bytes.len(), &mut err);
        if results.is_null() {
            assert!(!err.is_null(), "NULL results must come with an error");
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
            };
            simlin_error_free(err);
            Err((code, msg))
        } else {
            assert!(err.is_null(), "successful open must not set out_error");
            Ok(results)
        }
    }
}

fn open_vdf_fixture(path: &str) -> *mut SimlinResults {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    open_vdf(&bytes).unwrap_or_else(|(code, msg)| panic!("opening {path}: {code:?}: {msg}"))
}

fn var_names(results: *mut SimlinResults) -> Vec<String> {
    unsafe {
        let mut count = 0usize;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_results_get_var_count(results, &mut count, &mut err);
        assert!(err.is_null());

        let mut ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        let mut written = 0usize;
        simlin_results_get_var_names(results, ptrs.as_mut_ptr(), count, &mut written, &mut err);
        assert!(err.is_null());
        assert_eq!(written, count);

        ptrs.into_iter()
            .map(|p| {
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                simlin_free_string(p);
                s
            })
            .collect()
    }
}

fn series(results: *mut SimlinResults, name: &str) -> Vec<f64> {
    unsafe {
        let mut steps = 0usize;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_results_get_stepcount(results, &mut steps, &mut err);
        assert!(err.is_null());

        let c_name = CString::new(name).unwrap();
        let mut buf = vec![0.0f64; steps];
        let mut written = 0usize;
        simlin_results_get_series(
            results,
            c_name.as_ptr(),
            buf.as_mut_ptr(),
            steps,
            &mut written,
            &mut err,
        );
        assert!(err.is_null(), "get_series({name}) failed");
        assert_eq!(written, steps);
        buf
    }
}

#[test]
fn open_vdf_run_file_exposes_named_series() {
    let results = open_vdf_fixture("../../test/bobby/vdf/water/Current.vdf");

    unsafe {
        let mut steps = 0usize;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_results_get_stepcount(results, &mut steps, &mut err);
        assert!(err.is_null());
        assert_eq!(steps, 21);
    }

    let names = var_names(results);
    assert!(names.contains(&"time".to_string()));
    assert!(names.contains(&"water_level".to_string()));
    assert!(names.contains(&"gap".to_string()));
    // Sorted output is part of the contract (mirrors simlin_sim_get_var_names).
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);

    let time = series(results, "time");
    assert_eq!(time[0], 0.0);
    assert_eq!(time[20], 20.0);

    // Pinned Vensim outputs for the water model.
    let water_level = series(results, "water_level");
    assert_eq!(water_level[0], 0.0);
    assert!((water_level[10] - 0.9500000476837158).abs() < 1e-12);
    assert!((water_level[20] - 0.999951183795929).abs() < 1e-12);

    // The un-canonicalized display name resolves through the same lookup
    // rule the sim surface uses (Ident::new canonicalization).
    let water_level_display = series(results, "Water Level");
    assert_eq!(water_level, water_level_display);

    unsafe { simlin_results_unref(results) };
}

#[test]
fn open_vdf_dataset_file_exposes_named_series() {
    let results = open_vdf_fixture("../../test/bobby/vdf/econ/data.vdf");

    unsafe {
        let mut steps = 0usize;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_results_get_stepcount(results, &mut steps, &mut err);
        assert!(err.is_null());
        assert_eq!(steps, 225);
    }

    let names = var_names(results);
    assert!(names.contains(&"time".to_string()));
    assert!(names.contains(&"consumer_price_index".to_string()));

    // Pinned from the engine's test_dataset_vdf_extracts_reference_series.
    let time = series(results, "time");
    assert!((time[0] - 1990.0).abs() < 1e-6);
    assert!((time[224] - 2008.6700439453125).abs() < 1e-6);

    let cpi = series(results, "Consumer Price Index");
    assert!((cpi[0] - 127.4000015258789).abs() < 1e-6);
    assert!((cpi[224] - 218.7830047607422).abs() < 1e-6);

    let inflation = series(results, "Inflation Rate");
    assert!(inflation[0].is_nan());
    assert!((inflation[12] - 5.38116979598999).abs() < 1e-6);

    unsafe { simlin_results_unref(results) };
}

#[test]
fn open_vdf_rejects_malformed_input_without_crashing() {
    // Empty buffer.
    let (code, msg) = open_vdf(&[]).unwrap_err();
    assert_eq!(code, SimlinErrorCode::Generic);
    assert!(msg.contains("magic"), "unexpected message: {msg}");

    // Garbage bytes (wrong magic).
    let garbage = vec![0xABu8; 256];
    let (code, _) = open_vdf(&garbage).unwrap_err();
    assert_eq!(code, SimlinErrorCode::Generic);

    // Truncated real run file: several prefix lengths, all must error (or,
    // for long-enough prefixes, still parse) without crashing.
    let data = std::fs::read("../../test/bobby/vdf/water/Current.vdf").unwrap();
    for len in [4usize, 16, 100, 0x80, 1000, data.len() - 2] {
        if let Ok(results) = open_vdf(&data[..len]) {
            unsafe { simlin_results_unref(results) };
        }
    }

    // Truncated dataset file through the same auto-detecting entry point.
    let dataset = std::fs::read("../../test/bobby/vdf/econ/data.vdf").unwrap();
    for len in [4usize, 16, 100, 0x80, 1000, dataset.len() - 2] {
        if let Ok(results) = open_vdf(&dataset[..len]) {
            unsafe { simlin_results_unref(results) };
        }
    }

    // NULL data with non-zero length.
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let results = simlin_results_open_vdf(ptr::null(), 16, &mut err);
        assert!(results.is_null());
        assert!(!err.is_null());
        simlin_error_free(err);
    }

    // Zero-step run file: header time-point count (0x78) and the Time
    // block's u16 count both zeroed. This coordinated corruption slips past
    // the single-mutation sweep and used to reach an index panic in
    // build_results -- fatal under panic=abort, where catch_unwind cannot
    // help -- so it must surface as an error through the FFI.
    let mut zero_step = data.clone();
    let offset_table_start = u32::from_le_bytes(zero_step[0x60..0x64].try_into().unwrap()) as usize;
    let time_block = u32::from_le_bytes(
        zero_step[offset_table_start..offset_table_start + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    zero_step[0x78..0x7C].copy_from_slice(&0u32.to_le_bytes());
    zero_step[time_block..time_block + 2].copy_from_slice(&0u16.to_le_bytes());
    let (code, msg) = open_vdf(&zero_step).unwrap_err();
    assert_eq!(code, SimlinErrorCode::Generic);
    assert!(
        msg.contains("zero saved time points"),
        "unexpected message: {msg}"
    );
}

#[test]
fn results_handle_refcounting_and_null_args() {
    let results = open_vdf_fixture("../../test/bobby/vdf/water/Current.vdf");
    unsafe {
        // ref then double-unref must not free early or double-free.
        simlin_results_ref(results);
        simlin_results_unref(results);

        let mut count = 0usize;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_results_get_var_count(results, &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 10);

        // Unknown series -> DoesNotExist.
        let c_name = CString::new("no_such_series").unwrap();
        let mut buf = vec![0.0f64; 4];
        let mut written = 0usize;
        simlin_results_get_series(
            results,
            c_name.as_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
            &mut written,
            &mut err,
        );
        assert!(!err.is_null());
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::DoesNotExist);
        simlin_error_free(err);

        // NULL handle -> error, not crash.
        let mut err2: *mut SimlinError = ptr::null_mut();
        simlin_results_get_stepcount(ptr::null_mut(), &mut count, &mut err2);
        assert!(!err2.is_null());
        simlin_error_free(err2);

        simlin_results_unref(results);
    }
}
