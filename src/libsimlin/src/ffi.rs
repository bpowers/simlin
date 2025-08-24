// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! FFI type definitions for cbindgen

use std::os::raw::c_char;

/// Opaque project structure
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinProject {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque simulation structure  
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinSim {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Loop polarity for C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLoopPolarity {
    Reinforcing = 0,
    Balancing = 1,
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

/// Error detail structure containing error message and location
#[repr(C)]
pub struct SimlinErrorDetail {
    pub code: crate::SimlinErrorCode,
    pub message: *mut c_char,       // Optional error message (may be NULL)
    pub model_name: *mut c_char,    // Model where error occurred (may be NULL)
    pub variable_name: *mut c_char, // Variable where error occurred (may be NULL)
    pub start_offset: u16,          // Start offset in equation (0 if not applicable)
    pub end_offset: u16,            // End offset in equation (0 if not applicable)
}

/// Collection of error details
#[repr(C)]
pub struct SimlinErrorDetails {
    pub errors: *mut SimlinErrorDetail,
    pub count: usize,
}
