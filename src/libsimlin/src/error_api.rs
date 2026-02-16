// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Error inspection FFI functions.
//!
//! These functions allow C / WASM callers to inspect `SimlinError` objects:
//! converting error codes to strings, querying error details, and freeing
//! error objects.

use std::mem::size_of;
use std::os::raw::c_char;
use std::ptr;

use crate::ffi;
use crate::ffi_error::SimlinError;
use crate::{SimlinErrorCode, SimlinErrorDetail};

/// simlin_error_str returns a string representation of an error code.
/// The returned string must not be freed or modified.
///
/// Accepts a u32 discriminant rather than an enum to safely handle invalid values
/// from C/WASM callers. Returns "unknown_error" for invalid discriminants.
#[no_mangle]
pub extern "C" fn simlin_error_str(err: u32) -> *const c_char {
    let s: &'static str = match SimlinErrorCode::try_from(err) {
        Ok(SimlinErrorCode::NoError) => "no_error\0",
        Ok(SimlinErrorCode::DoesNotExist) => "does_not_exist\0",
        Ok(SimlinErrorCode::XmlDeserialization) => "xml_deserialization\0",
        Ok(SimlinErrorCode::VensimConversion) => "vensim_conversion\0",
        Ok(SimlinErrorCode::ProtobufDecode) => "protobuf_decode\0",
        Ok(SimlinErrorCode::InvalidToken) => "invalid_token\0",
        Ok(SimlinErrorCode::UnrecognizedEof) => "unrecognized_eof\0",
        Ok(SimlinErrorCode::UnrecognizedToken) => "unrecognized_token\0",
        Ok(SimlinErrorCode::ExtraToken) => "extra_token\0",
        Ok(SimlinErrorCode::UnclosedComment) => "unclosed_comment\0",
        Ok(SimlinErrorCode::UnclosedQuotedIdent) => "unclosed_quoted_ident\0",
        Ok(SimlinErrorCode::ExpectedNumber) => "expected_number\0",
        Ok(SimlinErrorCode::UnknownBuiltin) => "unknown_builtin\0",
        Ok(SimlinErrorCode::BadBuiltinArgs) => "bad_builtin_args\0",
        Ok(SimlinErrorCode::EmptyEquation) => "empty_equation\0",
        Ok(SimlinErrorCode::BadModuleInputDst) => "bad_module_input_dst\0",
        Ok(SimlinErrorCode::BadModuleInputSrc) => "bad_module_input_src\0",
        Ok(SimlinErrorCode::NotSimulatable) => "not_simulatable\0",
        Ok(SimlinErrorCode::BadTable) => "bad_table\0",
        Ok(SimlinErrorCode::BadSimSpecs) => "bad_sim_specs\0",
        Ok(SimlinErrorCode::NoAbsoluteReferences) => "no_absolute_references\0",
        Ok(SimlinErrorCode::CircularDependency) => "circular_dependency\0",
        Ok(SimlinErrorCode::ArraysNotImplemented) => "arrays_not_implemented\0",
        Ok(SimlinErrorCode::MultiDimensionalArraysNotImplemented) => {
            "multi_dimensional_arrays_not_implemented\0"
        }
        Ok(SimlinErrorCode::BadDimensionName) => "bad_dimension_name\0",
        Ok(SimlinErrorCode::BadModelName) => "bad_model_name\0",
        Ok(SimlinErrorCode::MismatchedDimensions) => "mismatched_dimensions\0",
        Ok(SimlinErrorCode::ArrayReferenceNeedsExplicitSubscripts) => {
            "array_reference_needs_explicit_subscripts\0"
        }
        Ok(SimlinErrorCode::DuplicateVariable) => "duplicate_variable\0",
        Ok(SimlinErrorCode::UnknownDependency) => "unknown_dependency\0",
        Ok(SimlinErrorCode::VariablesHaveErrors) => "variables_have_errors\0",
        Ok(SimlinErrorCode::UnitDefinitionErrors) => "unit_definition_errors\0",
        Ok(SimlinErrorCode::Generic) => "generic\0",
        Ok(SimlinErrorCode::UnitMismatch) => "unit_mismatch\0",
        Ok(SimlinErrorCode::BadOverride) => "bad_override\0",
        Err(()) => "unknown_error\0",
    };
    s.as_ptr() as *const c_char
}

/// Returns the size of the SimlinLoop struct in bytes.
///
/// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
#[no_mangle]
pub extern "C" fn simlin_sizeof_loop() -> usize {
    size_of::<ffi::SimlinLoop>()
}

/// Returns the size of the SimlinLink struct in bytes.
///
/// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
#[no_mangle]
pub extern "C" fn simlin_sizeof_link() -> usize {
    size_of::<ffi::SimlinLink>()
}

/// Returns the size of the SimlinErrorDetail struct in bytes.
///
/// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
#[no_mangle]
pub extern "C" fn simlin_sizeof_error_detail() -> usize {
    size_of::<SimlinErrorDetail>()
}

/// Returns the size of a pointer on the current platform.
///
/// Use this to validate ABI compatibility (expected 4 for wasm32).
#[no_mangle]
pub extern "C" fn simlin_sizeof_ptr() -> usize {
    size_of::<*const u8>()
}

/// # Safety
///
/// The pointer must have been created by a simlin function that returns a `*mut SimlinError`,
/// must not be null, and must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_free(err: *mut SimlinError) {
    if err.is_null() {
        return;
    }
    let _ = SimlinError::from_raw(err);
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_code(err: *const SimlinError) -> SimlinErrorCode {
    if err.is_null() {
        return SimlinErrorCode::Generic;
    }
    (*err).code()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned string pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_message(err: *const SimlinError) -> *const c_char {
    if err.is_null() {
        return ptr::null();
    }
    (*err).message_ptr()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_detail_count(err: *const SimlinError) -> usize {
    if err.is_null() {
        return 0;
    }
    (*err).detail_count()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned array pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_details(
    err: *const SimlinError,
) -> *const SimlinErrorDetail {
    if err.is_null() {
        return ptr::null();
    }
    (*err).details_ptr()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned detail pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_detail(
    err: *const SimlinError,
    index: usize,
) -> *const SimlinErrorDetail {
    if err.is_null() {
        return ptr::null();
    }
    (*err).detail_at(index)
}
