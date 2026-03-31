// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Re-exports error formatting from simlin-engine.
//!
//! The canonical implementation lives in `simlin_engine::errors`.
//! This module re-exports everything for backwards compatibility
//! with existing libsimlin callers.

pub use simlin_engine::errors::{
    collect_formatted_errors, format_diagnostic, format_diagnostic_with_datamodel,
    format_simulation_error, FormattedError, FormattedErrorKind, FormattedErrors, UnitErrorKind,
};

// Backwards compatibility alias for callers using the old name
pub use simlin_engine::errors::collect_formatted_errors as collect_formatted_issues_from_diagnostics;
