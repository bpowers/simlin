// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Type definitions for MDL to datamodel conversion.

use simlin_core::datamodel::{Dt, Equation, SimMethod, SimSpecs};

use crate::mdl::ast::FullEquation;

/// Errors that can occur during MDL to datamodel conversion.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ConvertError {
    /// Reader error during parsing
    Reader(crate::mdl::reader::ReaderError),
    /// Invalid subscript range specification
    InvalidRange(String),
    /// Cyclic dimension definition detected (e.g., DimA: DimB, DimB: DimA)
    CyclicDimensionDefinition(String),
    /// Other conversion error
    Other(String),
}

impl From<crate::mdl::reader::ReaderError> for ConvertError {
    fn from(e: crate::mdl::reader::ReaderError) -> Self {
        ConvertError::Reader(e)
    }
}

/// Type of variable determined during conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VariableType {
    Stock,
    Flow,
    Aux,
}

/// Information about a symbol collected during the first pass.
#[derive(Debug)]
pub(super) struct SymbolInfo<'input> {
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
    /// Alternate name for XMILE output (e.g., "DT" for "TIME STEP")
    pub alternate_name: Option<String>,
}

impl<'input> SymbolInfo<'input> {
    pub fn new() -> Self {
        SymbolInfo {
            equations: Vec::new(),
            var_type: VariableType::Aux,
            inflows: Vec::new(),
            outflows: Vec::new(),
            unwanted: false,
            alternate_name: None,
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
    pub dt: Option<f64>,
    pub save_step: Option<f64>,
    pub time_units: Option<String>,
}

impl SimSpecsBuilder {
    pub fn build(self) -> SimSpecs {
        SimSpecs {
            start: self.start.unwrap_or(0.0),
            stop: self.stop.unwrap_or(200.0),
            dt: self.dt.map(Dt::Dt).unwrap_or_default(),
            // Saveper defaults to dt if not specified (per xmutil behavior)
            save_step: self.save_step.or(self.dt).map(Dt::Dt),
            sim_method: SimMethod::Euler,
            // Default to "Months" to match xmutil
            time_units: self.time_units.or_else(|| Some("Months".to_string())),
        }
    }
}
