// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP-facing input/output types shared between tools.
//!
//! Task 3 will fold the remaining shared output types
//! (`LoopDominanceSummary`, `DominantPeriodOutput`, `ErrorOutput`, etc.)
//! into this module.  For now the only resident is [`SourceFormat`],
//! which the [`crate::access::ProjectAccess`] trait depends on.

/// Identifies how a model file was parsed so write-back can use the same
/// format.  `Xmile` covers `.stmx`, `.xmile`, `.xml`, and (read-only)
/// `.mdl` Vensim files; the JSON variants are distinguished by content
/// rather than extension (`models` vs `variables` at the top level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Xmile,
    NativeJson,
    SdaiJson,
}
