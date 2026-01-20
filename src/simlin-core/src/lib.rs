// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

pub mod common;
pub mod datamodel;
mod results;

// Re-export key types from common
pub use common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, CanonicalStr, DimensionName,
    ElementName, EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, IdentRef, Raw,
    RawDimensionName, RawElementName, RawIdent, Result, canonicalize,
};

// Re-export results types
pub use results::{Method, Results, Specs};
