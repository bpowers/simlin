// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The authoritative diagnostic payload shared by parsing, lowering, salsa,
//! formatting, and public adapters.

use crate::builtins::Loc;
use crate::common::{EquationError, Error, ErrorCode, ErrorKind, UnitError};

/// Whether a diagnostic prevents compilation or describes a degraded result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

impl DiagnosticSeverity {
    /// Every severity handled by diagnostic producers and adapters.
    pub const ALL: [Self; 2] = [Self::Error, Self::Warning];
}

/// The pipeline surface that raised a diagnostic.
///
/// This is deliberately independent of [`ErrorCode`]: one code can be raised
/// by more than one surface, while formatting and wire adapters still need to
/// distinguish an equation span from a model failure or unit conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticCategory {
    Import,
    Model,
    Variable,
    Equation,
    UnitDefinition,
    UnitConsistency,
    UnitInference,
    Assembly,
}

impl DiagnosticCategory {
    /// Every category handled by diagnostic producers and adapters.
    pub const ALL: [Self; 8] = [
        Self::Import,
        Self::Model,
        Self::Variable,
        Self::Equation,
        Self::UnitDefinition,
        Self::UnitConsistency,
        Self::UnitInference,
        Self::Assembly,
    ];

    /// Categories emitted by the incremental compilation diagnostic channel.
    ///
    /// `Import` errors are returned by format readers before a project can be
    /// synced, and `Variable` is an adapter category for direct engine errors;
    /// neither is accumulated by a compiler query. Their formatter arms remain
    /// part of [`Self::ALL`].
    pub const COMPILE_PIPELINE: [Self; 6] = [
        Self::Model,
        Self::Equation,
        Self::UnitDefinition,
        Self::UnitConsistency,
        Self::UnitInference,
        Self::Assembly,
    ];

    /// Whether this category belongs to parsing or checking units.
    pub const fn is_unit(self) -> bool {
        matches!(
            self,
            Self::UnitDefinition | Self::UnitConsistency | Self::UnitInference
        )
    }
}

/// An additional source implicated in a diagnostic.
///
/// Unit inference is the first producer with several sources. The vector on
/// [`Diagnostic`] preserves producer order; it is not a set, because the first
/// source is the deterministic comparison anchor used in the user-facing
/// explanation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiagnosticSource {
    pub variable: String,
    pub location: Option<Loc>,
}

/// One complete diagnostic from parse/lower through public collection.
///
/// A parsed or lowered [`crate::variable::Variable`] owns context-free values.
/// The database boundary calls [`Diagnostic::with_context`] exactly once before
/// emitting one. Direct database producers construct the same payload and call
/// the same method. `owner` distinguishes a synthesized helper's user-authored
/// parent from its physical `variable` name; `module_path` and `element` keep
/// identities structural rather than forcing them into prose.
#[salsa::accumulator]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub owner: Option<String>,
    pub module_path: Vec<String>,
    pub element: Option<String>,
    pub location: Option<Loc>,
    pub related: Vec<DiagnosticSource>,
    pub code: ErrorCode,
    pub category: DiagnosticCategory,
    /// Optional presentation-focused explanation. `details` always retains
    /// the producer's complete reason; a formatter may prefer this shorter
    /// modeler-facing text without destroying the original payload.
    pub display_details: Option<String>,
    pub details: Option<String>,
    pub severity: DiagnosticSeverity,
    context_attached: bool,
}

impl Diagnostic {
    fn context_free(
        code: ErrorCode,
        category: DiagnosticCategory,
        location: Option<Loc>,
        details: Option<String>,
        related: Vec<DiagnosticSource>,
        severity: DiagnosticSeverity,
    ) -> Self {
        Self {
            model: String::new(),
            variable: None,
            owner: None,
            module_path: Vec::new(),
            element: None,
            location,
            related,
            code,
            category,
            display_details: None,
            details,
            severity,
            context_attached: false,
        }
    }

    /// Convert a local equation failure without dropping its span or reason.
    pub fn equation(error: EquationError, severity: DiagnosticSeverity) -> Self {
        Self::context_free(
            error.code,
            DiagnosticCategory::Equation,
            Some(Loc {
                start: error.start,
                end: error.end,
            }),
            error.details,
            Vec::new(),
            severity,
        )
    }

    /// Convert a local unit failure without dropping its kind, sources, span,
    /// or reason.
    pub fn unit(error: UnitError, severity: DiagnosticSeverity) -> Self {
        match error {
            UnitError::DefinitionError(error) => Self::context_free(
                error.code,
                DiagnosticCategory::UnitDefinition,
                Some(Loc {
                    start: error.start,
                    end: error.end,
                }),
                error.details,
                Vec::new(),
                severity,
            ),
            UnitError::ConsistencyError(code, location, details) => Self::context_free(
                code,
                DiagnosticCategory::UnitConsistency,
                Some(location),
                details,
                Vec::new(),
                severity,
            ),
            UnitError::InferenceError {
                code,
                sources,
                details,
            } => Self::context_free(
                code,
                DiagnosticCategory::UnitInference,
                None,
                details,
                sources
                    .into_iter()
                    .map(|(variable, location)| DiagnosticSource { variable, location })
                    .collect(),
                severity,
            ),
        }
    }

    /// Convert a non-equation engine error without losing its category.
    pub fn engine(error: Error, severity: DiagnosticSeverity) -> Self {
        let category = match error.kind {
            ErrorKind::Import => DiagnosticCategory::Import,
            ErrorKind::Model => DiagnosticCategory::Model,
            ErrorKind::Variable => DiagnosticCategory::Variable,
            ErrorKind::Simulation => DiagnosticCategory::Assembly,
        };
        Self::context_free(
            error.code,
            category,
            None,
            error.details,
            Vec::new(),
            severity,
        )
    }

    /// Convert an engine error that is raised as a model-level compilation
    /// failure regardless of its low-level [`ErrorKind`].
    ///
    /// Table construction is the live producer: its low-level implementation
    /// reports a simulation-kind error, while the compilation failure belongs
    /// to the declaring model.
    pub fn model(error: Error, severity: DiagnosticSeverity) -> Self {
        Self::context_free(
            error.code,
            DiagnosticCategory::Model,
            None,
            error.details,
            Vec::new(),
            severity,
        )
    }

    /// Construct a compiler/assembly diagnostic from its textual reason.
    pub fn assembly(details: impl Into<String>, severity: DiagnosticSeverity) -> Self {
        Self::context_free(
            ErrorCode::NotSimulatable,
            DiagnosticCategory::Assembly,
            None,
            Some(details.into()),
            Vec::new(),
            severity,
        )
    }

    /// Attach the source-model context once, at the database boundary.
    #[must_use]
    pub fn with_context(mut self, model: impl Into<String>, variable: Option<String>) -> Self {
        assert!(
            !self.context_attached,
            "diagnostic context must be attached exactly once"
        );
        self.model = model.into();
        self.variable = variable;
        self.context_attached = true;
        self
    }

    /// Record the user-authored variable that owns a physical implicit helper.
    #[must_use]
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Record an element identity without encoding it into the reason string.
    #[must_use]
    pub fn with_element(mut self, element: impl Into<String>) -> Self {
        self.element = Some(element.into());
        self
    }

    /// Record the module-instance path that led to the source model.
    #[must_use]
    pub fn with_module_path(mut self, module_path: Vec<String>) -> Self {
        self.module_path = module_path;
        self
    }

    /// Replace the ordered related-source list.
    #[must_use]
    pub fn with_related(mut self, related: Vec<DiagnosticSource>) -> Self {
        self.related = related;
        self
    }

    /// Supply a concise presentation reason while retaining `details` intact.
    #[must_use]
    pub fn with_display_details(mut self, details: impl Into<String>) -> Self {
        self.display_details = Some(details.into());
        self
    }

    /// The modeler-facing reason, preferring a deliberately concise rendering
    /// while the complete producer detail remains available in `details`.
    pub fn reason(&self) -> Option<&str> {
        self.display_details.as_deref().or(self.details.as_deref())
    }

    /// The assembly reason when this diagnostic was produced by compiler or
    /// simulation assembly.
    pub fn assembly_reason(&self) -> Option<&str> {
        (self.category == DiagnosticCategory::Assembly)
            .then(|| self.reason())
            .flatten()
    }

    /// Whether this diagnostic has a particular category and code.
    pub fn is(&self, category: DiagnosticCategory, code: ErrorCode) -> bool {
        self.category == category && self.code == code
    }

    /// Emit a fully-attributed diagnostic into salsa's accumulator.
    pub fn emit<Db>(self, db: &Db)
    where
        Db: ?Sized + salsa::Database,
    {
        use salsa::Accumulator;
        assert!(
            self.context_attached,
            "a diagnostic must receive model/variable context before emission"
        );
        self.accumulate(db);
    }
}
