// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one diagnostic payload, from a raising site to `collect_all_diagnostics`.
//!
//! A raising site produces a typed error -- an `EquationError`, an `Error`, a
//! `UnitError`, or assembly's prose -- and nothing downstream translates it:
//! [`DiagnosticError`] is the sum of those types, and [`Diagnostic`], the
//! salsa accumulator, is that sum plus the context it is reported under.
//! Context is attached exactly once, by type: a `Variable` carries the
//! context-free `DiagnosticError`s its parse and lowering raised
//! (`Variable::diagnostics`), and a `Diagnostic` is built at the salsa
//! layer's raising sites, where the model, the variable and the severity are
//! known; nothing between a site and the drain re-attaches context, and
//! nothing asserts it at run time.
//! Consumers read the payload through its projections ([`Diagnostic::code`],
//! [`Diagnostic::category`], [`Diagnostic::location`], [`Diagnostic::reason`],
//! [`Diagnostic::is`]) rather than by matching the sum, and
//! `errors::FormattedError` is its one presentation adapter.

use crate::builtins::Loc;
use crate::common::{EquationError, Error, ErrorCode, UnitError};

/// Whether a diagnostic stops compilation or describes a degraded result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

/// The pipeline surface a diagnostic was raised on: [`DiagnosticError`]'s
/// arms, with `Unit` split by `UnitError`'s.
///
/// The formatter's presentation kind, libsimlin's `SimlinUnitErrorKind` and a
/// test's arm predicate all select on this one enumeration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticCategory {
    /// An `EquationError`: a parse, lowering or fragment-compile failure of
    /// one equation, with a span into its text.
    Equation,
    /// An `Error` raised against the model as a whole (a cycle, a duplicate
    /// name, a bad table) or as an advisory about it.
    Model,
    /// A `<units>` declaration or string that does not parse.
    UnitDefinition,
    /// An equation whose computed units disagree with its declared ones.
    UnitConsistency,
    /// A contradiction in the model-wide unit-constraint system.
    UnitInference,
    /// Codegen or an analysis overlay refusing a construct, in prose.
    Assembly,
}

/// The typed error a raising site produced, exactly as it produced it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
    /// A codegen refusal or an LTM advisory: neither has a span or a code of
    /// its own, so the emitter's message IS the payload and the code every
    /// consumer reads is `NotSimulatable`.
    Assembly(String),
}

impl DiagnosticError {
    /// The class of failure, whichever arm carries it.
    pub fn code(&self) -> ErrorCode {
        match self {
            DiagnosticError::Equation(err) => err.code,
            DiagnosticError::Model(err) => err.code,
            DiagnosticError::Unit(UnitError::DefinitionError(err)) => err.code,
            DiagnosticError::Unit(UnitError::ConsistencyError(code, _, _))
            | DiagnosticError::Unit(UnitError::InferenceError { code, .. }) => *code,
            DiagnosticError::Assembly(_) => ErrorCode::NotSimulatable,
        }
    }

    pub fn category(&self) -> DiagnosticCategory {
        match self {
            DiagnosticError::Equation(_) => DiagnosticCategory::Equation,
            DiagnosticError::Model(_) => DiagnosticCategory::Model,
            DiagnosticError::Unit(UnitError::DefinitionError(_)) => {
                DiagnosticCategory::UnitDefinition
            }
            DiagnosticError::Unit(UnitError::ConsistencyError(..)) => {
                DiagnosticCategory::UnitConsistency
            }
            DiagnosticError::Unit(UnitError::InferenceError { .. }) => {
                DiagnosticCategory::UnitInference
            }
            DiagnosticError::Assembly(_) => DiagnosticCategory::Assembly,
        }
    }

    /// The span the raising site pointed at, into the equation (or the unit
    /// string) of the variable the diagnostic names; an inference error's is
    /// its first source's.
    pub fn location(&self) -> Option<Loc> {
        match self {
            DiagnosticError::Equation(err)
            | DiagnosticError::Unit(UnitError::DefinitionError(err)) => Some(Loc {
                start: err.start,
                end: err.end,
            }),
            DiagnosticError::Unit(UnitError::ConsistencyError(_, loc, _)) => Some(*loc),
            DiagnosticError::Unit(UnitError::InferenceError { sources, .. }) => {
                sources.first().and_then(|(_, loc)| *loc)
            }
            DiagnosticError::Model(_) | DiagnosticError::Assembly(_) => None,
        }
    }

    /// The reason the raising site wrote, when the code and the span do not
    /// already carry it (a parse error's reason is its snippet).
    pub fn reason(&self) -> Option<&str> {
        match self {
            DiagnosticError::Equation(err)
            | DiagnosticError::Unit(UnitError::DefinitionError(err)) => err.details.as_deref(),
            DiagnosticError::Model(err) => err.details.as_deref(),
            DiagnosticError::Unit(UnitError::ConsistencyError(_, _, details))
            | DiagnosticError::Unit(UnitError::InferenceError { details, .. }) => {
                details.as_deref()
            }
            DiagnosticError::Assembly(message) => Some(message),
        }
    }
}

/// One diagnostic as `collect_all_diagnostics` reports it: the typed error
/// plus the context it is reported under. The salsa accumulator.
///
/// `model` is empty for a project-level diagnostic (a unit declaration, the
/// macro set). `variable` is the name the error is filed under -- for a
/// generated helper its physical `$⁚…` name, which is the identity the
/// compiler, the layout and de-duplication use -- and `owner` is the
/// variable that helper was synthesized for (a user variable, or for an LTM
/// helper the synthetic link score), the name a consumer presents the row
/// under; both are `None` for a model-level row.
#[salsa::accumulator]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub owner: Option<String>,
    pub severity: DiagnosticSeverity,
    pub error: DiagnosticError,
}

impl Diagnostic {
    pub fn code(&self) -> ErrorCode {
        self.error.code()
    }

    pub fn category(&self) -> DiagnosticCategory {
        self.error.category()
    }

    pub fn location(&self) -> Option<Loc> {
        self.error.location()
    }

    pub fn reason(&self) -> Option<&str> {
        self.error.reason()
    }

    /// Whether this diagnostic is `code` raised on `category`'s surface.
    pub fn is(&self, category: DiagnosticCategory, code: ErrorCode) -> bool {
        self.category() == category && self.code() == code
    }
}
