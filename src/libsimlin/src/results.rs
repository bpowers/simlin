// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Standalone simulation-results FFI.
//!
//! `SimlinResults` is an opaque handle around an `engine::Results` table that
//! is not tied to a project/model/sim. Today the only producer is
//! [`simlin_results_open_vdf`], which imports a Vensim VDF file (run,
//! sensitivity-run, or dataset container, auto-detected by magic). The
//! accessors deliberately mirror the `simlin_sim_get_*` result readers
//! (step count, variable names, per-variable series) so callers consume the
//! two surfaces identically.

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::{self as engine, vdf};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::ptr;
use std::sync::atomic::AtomicUsize;

use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::{
    clear_out_error, drop_c_string, require_results, results_ref, results_unref, store_error,
    SimlinErrorCode, SimlinResults,
};

/// Parse VDF bytes into an engine `Results` table, dispatching on the file
/// magic: run/sensitivity containers go through
/// `VdfFile::to_results_via_records`, dataset containers through
/// `VdfDatasetFile::extract_data`.
fn parse_vdf_results(bytes: Vec<u8>) -> Result<engine::Results, String> {
    match vdf::probe_vdf_kind(&bytes) {
        Some(vdf::VdfKind::SimulationResults) | Some(vdf::VdfKind::SensitivityRun) => {
            let file = vdf::VdfFile::parse(bytes).map_err(|err| format!("parsing VDF: {err}"))?;
            file.to_results_via_records()
                .map_err(|err| format!("extracting VDF results: {err}"))
        }
        Some(vdf::VdfKind::Dataset) => {
            let file = vdf::VdfDatasetFile::parse(bytes)
                .map_err(|err| format!("parsing dataset VDF: {err}"))?;
            let data = file
                .extract_data()
                .map_err(|err| format!("extracting dataset VDF series: {err}"))?;
            Ok(data.to_results())
        }
        None => Err("not a VDF file (unrecognized magic bytes)".to_string()),
    }
}

/// Opens a Vensim VDF (binary simulation data) file from a byte buffer.
///
/// Auto-detects the container kind from the file magic: simulation-run files
/// (`0x52`), sensitivity-run files (`0x53`), and dataset files (`0x41`) are
/// all supported. On success returns a `SimlinResults` handle (release with
/// `simlin_results_unref`); on failure returns NULL with an error stored in
/// `out_error`.
///
/// Malformed input reports an error rather than crashing: the engine's VDF
/// readers are total on arbitrary bytes (pinned by the engine's
/// truncation/corruption sweep test), and the `catch_unwind` here is
/// defense-in-depth for unwind builds (release builds compile with
/// panic=abort, where unwinding never starts).
///
/// # Safety
/// - `data` must point to `len` valid bytes (may be NULL only when `len` is 0)
#[no_mangle]
pub unsafe extern "C" fn simlin_results_open_vdf(
    data: *const u8,
    len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinResults {
    clear_out_error(out_error);
    if data.is_null() && len > 0 {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL when len > 0"),
        );
        return ptr::null_mut();
    }
    let bytes = if len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(data, len).to_vec()
    };

    let parsed =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_vdf_results(bytes)));
    match parsed {
        Ok(Ok(results)) => Box::into_raw(Box::new(SimlinResults {
            results,
            ref_count: AtomicUsize::new(1),
        })),
        Ok(Err(message)) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(message),
            );
            ptr::null_mut()
        }
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("internal error: panic while parsing VDF data"),
            );
            ptr::null_mut()
        }
    }
}

/// Increments the reference count of a results handle
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
#[no_mangle]
pub unsafe extern "C" fn simlin_results_ref(results: *mut SimlinResults) {
    results_ref(results);
}

/// Decrements the reference count and frees the results handle if it reaches zero
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
#[no_mangle]
pub unsafe extern "C" fn simlin_results_unref(results: *mut SimlinResults) {
    results_unref(results);
}

/// Gets the number of time steps in the results
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
#[no_mangle]
pub unsafe extern "C" fn simlin_results_get_stepcount(
    results: *mut SimlinResults,
    out_count: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_count.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_count pointer must not be NULL"),
        );
        return;
    }
    let results_ref = ffi_try!(out_error, require_results(results));
    *out_count = results_ref.results.step_count;
}

/// Gets the number of named series in the results (including `time`)
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
#[no_mangle]
pub unsafe extern "C" fn simlin_results_get_var_count(
    results: *mut SimlinResults,
    out_count: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_count.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_count pointer must not be NULL"),
        );
        return;
    }
    let results_ref = ffi_try!(out_error, require_results(results));
    *out_count = results_ref.results.offsets.len();
}

/// Gets the (sorted) names of the series in the results.
///
/// Call with `max == 0` to query the count without copying names.
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_results_get_var_names(
    results: *mut SimlinResults,
    result: *mut *mut c_char,
    max: usize,
    out_written: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_written.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_written pointer must not be NULL"),
        );
        return;
    }
    let results_ref = ffi_try!(out_error, require_results(results));

    let mut names_vec: Vec<&str> = results_ref
        .results
        .offsets
        .keys()
        .map(|k| k.as_str())
        .collect();

    if max == 0 {
        *out_written = names_vec.len();
        return;
    }
    if result.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("result pointer must not be NULL when max > 0"),
        );
        return;
    }

    names_vec.sort_unstable();

    let count = names_vec.len().min(max);
    let mut allocated: Vec<*mut c_char> = Vec::with_capacity(count);
    for (i, name) in names_vec.iter().take(count).enumerate() {
        let c_string = match CString::new(*name) {
            Ok(s) => s,
            Err(_) => {
                for allocated_ptr in allocated {
                    drop_c_string(allocated_ptr);
                }
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "series name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        let raw = c_string.into_raw();
        allocated.push(raw);
        *result.add(i) = raw;
    }

    *out_written = count;
}

/// Gets the time series for a named variable in the results
///
/// # Safety
/// - `results` must be a valid pointer to a SimlinResults
/// - `name` must be a valid C string
/// - `results_ptr` must point to allocated memory of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_results_get_series(
    results: *mut SimlinResults,
    name: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
    out_written: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_written.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_written pointer must not be NULL"),
        );
        return;
    }
    if results_ptr.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("results pointer must not be NULL"),
        );
        return;
    }
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let results_ref = ffi_try!(out_error, require_results(results));
    let raw_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    // Most VDF column keys are canonicalized idents, but a few are stored
    // raw (`#`-prefixed stdlib-call signatures use `from_str_unchecked`
    // because canonicalization would collapse them), so fall back to a
    // verbatim lookup when the canonical one misses.
    let table = &results_ref.results;
    let offset = table
        .offsets
        .get(&Ident::<Canonical>::new(raw_name))
        .or_else(|| {
            table
                .offsets
                .get(&Ident::<Canonical>::from_str_unchecked(raw_name))
        })
        .copied();
    let Some(offset) = offset else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist)
                .with_message(format!("series '{raw_name}' not found in results")),
        );
        return;
    };

    let count = std::cmp::min(table.step_count, len);
    for (i, row) in table.iter().take(count).enumerate() {
        *results_ptr.add(i) = row[offset];
    }
    *out_written = count;
}
