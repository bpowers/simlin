// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Edits to a project: the patches a host or an agent lands, the plans of a
//! person's gestures on a diagram, and a variable's equations as a host draws
//! them.

#![forbid(unsafe_code)]

pub mod editing;
pub mod equation_view;
pub mod patch;
mod patch_dimensions;
mod patch_fields;
#[cfg(test)]
mod patch_sharing_tests;

pub use self::patch::{
    PatchReport, Renamed, VariableRef, apply_patch, apply_patch_reporting, is_view_only_patch,
};
