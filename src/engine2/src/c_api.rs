// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! C API type definitions specifically for header generation

use std::os::raw::c_char;

/// Error codes for the C API
#[repr(C)]
pub enum SimlinErrorCode {
    SIMLIN_ERR_NO_ERROR = 0,
    SIMLIN_ERR_DOES_NOT_EXIST = 1,
    SIMLIN_ERR_XML_DESERIALIZATION = 2,
    SIMLIN_ERR_VENSIM_CONVERSION = 3,
    SIMLIN_ERR_PROTOBUF_DECODE = 4,
    SIMLIN_ERR_INVALID_TOKEN = 5,
    SIMLIN_ERR_UNRECOGNIZED_EOF = 6,
    SIMLIN_ERR_UNRECOGNIZED_TOKEN = 7,
    SIMLIN_ERR_EXTRA_TOKEN = 8,
    SIMLIN_ERR_UNCLOSED_COMMENT = 9,
    SIMLIN_ERR_UNCLOSED_QUOTED_IDENT = 10,
    SIMLIN_ERR_EXPECTED_NUMBER = 11,
    SIMLIN_ERR_UNKNOWN_BUILTIN = 12,
    SIMLIN_ERR_BAD_BUILTIN_ARGS = 13,
    SIMLIN_ERR_EMPTY_EQUATION = 14,
    SIMLIN_ERR_BAD_MODULE_INPUT_DST = 15,
    SIMLIN_ERR_BAD_MODULE_INPUT_SRC = 16,
    SIMLIN_ERR_NOT_SIMULATABLE = 17,
    SIMLIN_ERR_BAD_TABLE = 18,
    SIMLIN_ERR_BAD_SIM_SPECS = 19,
    SIMLIN_ERR_NO_ABSOLUTE_REFERENCES = 20,
    SIMLIN_ERR_CIRCULAR_DEPENDENCY = 21,
    SIMLIN_ERR_ARRAYS_NOT_IMPLEMENTED = 22,
    SIMLIN_ERR_MULTI_DIMENSIONAL_ARRAYS_NOT_IMPLEMENTED = 23,
    SIMLIN_ERR_BAD_DIMENSION_NAME = 24,
    SIMLIN_ERR_BAD_MODEL_NAME = 25,
    SIMLIN_ERR_MISMATCHED_DIMENSIONS = 26,
    SIMLIN_ERR_ARRAY_REFERENCE_NEEDS_EXPLICIT_SUBSCRIPTS = 27,
    SIMLIN_ERR_DUPLICATE_VARIABLE = 28,
    SIMLIN_ERR_UNKNOWN_DEPENDENCY = 29,
    SIMLIN_ERR_VARIABLES_HAVE_ERRORS = 30,
    SIMLIN_ERR_UNIT_DEFINITION_ERRORS = 31,
    SIMLIN_ERR_GENERIC = 32,
}

/// Loop polarity for C API
#[repr(C)]
pub enum SimlinLoopPolarity {
    SIMLIN_LOOP_REINFORCING = 0,
    SIMLIN_LOOP_BALANCING = 1,
}

/// Opaque project structure
#[repr(C)]
pub struct SimlinProject_s {
    _private: [u8; 0],
}

/// Opaque simulation structure  
#[repr(C)]
pub struct SimlinSim_s {
    _private: [u8; 0],
}

/// A single feedback loop
#[repr(C)]
pub struct SimlinLoop {
    pub id: *mut c_char,
    pub variables: *mut *mut c_char,
    pub var_count: usize,
    pub polarity: SimlinLoopPolarity,
}

/// List of loops returned by analysis
#[repr(C)]
pub struct SimlinLoops {
    pub loops: *mut SimlinLoop,
    pub count: usize,
}