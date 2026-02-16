// Copyright 2026 The Simlin Authors. All rights reserved.
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

/// Opaque model structure
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinModel {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque error structure returned by the API
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinError {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Loop polarity for C API
#[repr(C)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimlinLoopPolarity {
    Reinforcing = 0,
    Balancing = 1,
    Undetermined = 2,
}

/// Link polarity for C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLinkPolarity {
    Positive = 0,
    Negative = 1,
    Unknown = 2,
}

/// JSON format specifier for C API
#[repr(C)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimlinJsonFormat {
    Native = 0,
    Sdai = 1,
}

impl TryFrom<u32> for SimlinJsonFormat {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(SimlinJsonFormat::Native),
            1 => Ok(SimlinJsonFormat::Sdai),
            _ => Err(()),
        }
    }
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

/// Single causal link structure
#[repr(C)]
pub struct SimlinLink {
    pub from: *mut c_char,
    pub to: *mut c_char,
    pub polarity: SimlinLinkPolarity,
    pub score: *mut f64,
    pub score_len: usize,
}

/// Collection of links
#[repr(C)]
pub struct SimlinLinks {
    pub links: *mut SimlinLink,
    pub count: usize,
}
