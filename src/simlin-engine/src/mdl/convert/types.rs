// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Type definitions for MDL to datamodel conversion.

use crate::datamodel::{Dt, Equation, SimMethod, SimSpecs};

use crate::mdl::ast::FullEquation;

/// Errors that can occur during MDL to datamodel conversion.
#[derive(Debug)]
pub enum ConvertError {
    /// Reader error during parsing
    Reader(crate::mdl::reader::ReaderError),
    /// View parsing error
    View(crate::mdl::view::ViewError),
    /// Invalid subscript range specification
    InvalidRange(String),
    /// Cyclic dimension definition detected (e.g., DimA: DimB, DimB: DimA)
    CyclicDimensionDefinition(String),
    /// External data resolution failure (e.g. missing CSV, bad cell reference)
    Import(crate::common::Error),
    /// Generic error with a descriptive message
    Other(String),
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::Reader(e) => write!(f, "reader error: {}", e),
            ConvertError::View(e) => write!(f, "view error: {}", e),
            ConvertError::InvalidRange(s) => write!(f, "invalid subscript range: {}", s),
            ConvertError::CyclicDimensionDefinition(s) => {
                write!(f, "cyclic dimension definition: {}", s)
            }
            ConvertError::Import(e) => write!(f, "import error: {}", e),
            ConvertError::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConvertError::Reader(e) => Some(e),
            ConvertError::View(e) => Some(e),
            ConvertError::Import(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::mdl::reader::ReaderError> for ConvertError {
    fn from(e: crate::mdl::reader::ReaderError) -> Self {
        ConvertError::Reader(e)
    }
}

impl From<crate::mdl::view::ViewError> for ConvertError {
    fn from(e: crate::mdl::view::ViewError) -> Self {
        ConvertError::View(e)
    }
}

impl From<crate::common::Error> for ConvertError {
    fn from(e: crate::common::Error) -> Self {
        ConvertError::Import(e)
    }
}

/// Type of variable determined during conversion.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VariableType {
    Stock,
    Flow,
    Aux,
}

/// Information about a symbol collected during the first pass.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct SymbolInfo<'input> {
    /// The parsed equation(s) for this symbol
    pub equations: Vec<FullEquation<'input>>,
    /// Detected variable type
    pub var_type: VariableType,
    /// For stocks: list of inflow variable names
    pub inflows: Vec<String>,
    /// For stocks: list of outflow variable names
    pub outflows: Vec<String>,
    /// Whether this is a "unwanted" variable (control var)
    pub unwanted: bool,
}

impl<'input> SymbolInfo<'input> {
    pub fn new() -> Self {
        SymbolInfo {
            equations: Vec::new(),
            var_type: VariableType::Aux,
            inflows: Vec::new(),
            outflows: Vec::new(),
            unwanted: false,
        }
    }
}

/// A synthetic flow variable generated for stocks with non-decomposable rates.
pub(super) struct SyntheticFlow {
    /// Canonical name of the flow
    pub name: String,
    /// The full equation (Scalar, ApplyToAll, or Arrayed)
    pub equation: Equation,
}

/// Builder for SimSpecs extracted from control variables.
#[derive(Default)]
pub(super) struct SimSpecsBuilder {
    pub start: Option<f64>,
    pub stop: Option<f64>,
    pub dt: Option<Dt>,
    /// None when the save step follows the time step: no SAVEPER, one that
    /// is `TIME STEP`, or one that is not a constant.
    pub save_step: Option<Dt>,
    pub time_units: Option<String>,
    pub sim_method: Option<SimMethod>,
}

impl SimSpecsBuilder {
    pub fn build(self) -> SimSpecs {
        SimSpecs {
            start: self.start.unwrap_or(0.0),
            stop: self.stop.unwrap_or(200.0),
            dt: self.dt.unwrap_or_default(),
            // A SAVEPER that is a constant is that number. Any other SAVEPER
            // (`TIME STEP` itself, one that is not a constant, or none)
            // follows the time step, which the datamodel says with no save
            // step, so the writer writes it back as `TIME STEP` and a number
            // as that number: a save never changes which one the file means.
            save_step: self.save_step,
            sim_method: self.sim_method.unwrap_or(SimMethod::Euler),
            // Default to "Months" to match xmutil
            time_units: self.time_units.or_else(|| Some("Months".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::view::ViewError;

    #[test]
    fn test_convert_error_display_invalid_range() {
        let err = ConvertError::InvalidRange("bad range".to_string());
        assert_eq!(format!("{}", err), "invalid subscript range: bad range");
    }

    #[test]
    fn test_convert_error_display_cyclic_dimension() {
        let err = ConvertError::CyclicDimensionDefinition("DimA".to_string());
        assert_eq!(format!("{}", err), "cyclic dimension definition: DimA");
    }

    #[test]
    fn test_convert_error_display_view() {
        let err = ConvertError::View(ViewError::UnexpectedEndOfInput);
        assert_eq!(format!("{}", err), "view error: Unexpected end of input");
    }

    #[test]
    fn test_convert_error_display_reader() {
        let err = ConvertError::Reader(crate::mdl::reader::ReaderError::EofInsideMacro);
        assert_eq!(
            format!("{}", err),
            "reader error: unexpected end of file inside macro"
        );
    }

    #[test]
    fn test_convert_error_source_chains() {
        use std::error::Error;

        let view_err = ConvertError::View(ViewError::UnexpectedEndOfInput);
        assert!(view_err.source().is_some());

        let import_err = ConvertError::Import(crate::common::Error::new(
            crate::common::ErrorKind::Import,
            crate::common::ErrorCode::Generic,
            Some("test".to_string()),
        ));
        assert!(import_err.source().is_some());
        assert!(format!("{}", import_err).contains("import error"));

        let range_err = ConvertError::InvalidRange("x".to_string());
        assert!(range_err.source().is_none());
    }
}
