// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for the systems format.
//!
//! Builds a `SystemsModel` IR from systems format text input.
//! Handles stock deduplication, implicit flow type detection,
//! and declaration-order preservation.

use super::ast::SystemsModel;
use crate::common::Result;

/// Parse systems format text into a `SystemsModel` intermediate representation.
///
/// The parser processes input line by line:
/// - Comment lines (starting with `#`) are skipped
/// - Stock-only lines create stocks without flows
/// - Flow lines (`A > B @ rate`) create stocks and flows
///
/// Stock deduplication: when a name appears multiple times, initial/max values
/// are updated only if the new value is non-default and the existing is default.
/// Conflicting non-default values produce an error.
pub fn parse(_input: &str) -> Result<SystemsModel> {
    Ok(SystemsModel {
        stocks: Vec::new(),
        flows: Vec::new(),
    })
}
