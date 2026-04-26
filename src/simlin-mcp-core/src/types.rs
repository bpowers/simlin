// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! MCP-facing input/output types shared between tools.
//!
//! These types live in the core crate so both the stdio binary and the
//! Phase 6 HTTP host serialise tool responses byte-for-byte identically.
//! All `#[serde(rename_all = "camelCase")]` attributes are deliberate
//! wire-format choices preserved from the pre-refactor `simlin-mcp`.

use serde::Serialize;

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

/// Rounds a float to 3 significant figures via scientific-notation round-trip.
/// Mirrors Go's `strconv.FormatFloat(v, 'g', 3, 64)` behavior.
fn round_sig_figs_3(v: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let s = format!("{:.2e}", v);
    s.parse::<f64>().unwrap_or(v)
}

/// Serializes an importance array with values rounded to 3 significant figures,
/// reducing token count in MCP tool output.
fn serialize_importance<S: serde::Serializer>(
    values: &[f64],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(values.len()))?;
    for &v in values {
        seq.serialize_element(&round_sig_figs_3(v))?;
    }
    seq.end()
}

/// Per-loop dominance summary included in tool output.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopDominanceSummary {
    pub loop_id: String,
    pub name: Option<String>,
    pub polarity: String,
    pub variables: Vec<String>,
    #[serde(serialize_with = "serialize_importance")]
    pub importance: Vec<f64>,
}

impl From<simlin_engine::analysis::LoopSummary> for LoopDominanceSummary {
    fn from(ls: simlin_engine::analysis::LoopSummary) -> Self {
        Self {
            loop_id: ls.loop_id,
            name: ls.name,
            polarity: ls.polarity,
            variables: ls.variables,
            importance: ls.importance,
        }
    }
}

/// A time interval during which specific loops dominate model behavior.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DominantPeriodOutput {
    pub dominant_loops: Vec<String>,
    pub start_time: f64,
    pub end_time: f64,
}

impl From<simlin_engine::layout::metadata::DominantPeriod> for DominantPeriodOutput {
    fn from(dp: simlin_engine::layout::metadata::DominantPeriod) -> Self {
        Self {
            dominant_loops: dp.dominant_loops,
            start_time: dp.start,
            end_time: dp.end,
        }
    }
}

/// Structured error detail included in EditModel error responses.
///
/// Converts engine `FormattedError` into a serializable type suitable for
/// MCP structured content, so LLM clients can programmatically inspect
/// what went wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorOutput {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_name: Option<String>,
    pub kind: String,
}

impl From<&simlin_engine::errors::FormattedError> for ErrorOutput {
    fn from(fe: &simlin_engine::errors::FormattedError) -> Self {
        use simlin_engine::errors::FormattedErrorKind;
        let kind = match fe.kind {
            FormattedErrorKind::Project => "project",
            FormattedErrorKind::Model => "model",
            FormattedErrorKind::Variable => "variable",
            FormattedErrorKind::Units => "units",
            FormattedErrorKind::Simulation => "simulation",
        };
        Self {
            code: fe.code.to_string(),
            message: fe.message.clone().unwrap_or_default(),
            model_name: fe.model_name.clone(),
            variable_name: fe.variable_name.clone(),
            kind: kind.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_sig_figs_3_basic() {
        assert_eq!(round_sig_figs_3(2.449215777949112), 2.45);
    }

    #[test]
    fn round_sig_figs_3_zero() {
        assert_eq!(round_sig_figs_3(0.0), 0.0);
    }

    #[test]
    fn round_sig_figs_3_very_small() {
        assert_eq!(round_sig_figs_3(0.000004781283), 4.78e-6);
    }

    #[test]
    fn round_sig_figs_3_large() {
        assert_eq!(round_sig_figs_3(25.189), 25.2);
    }

    #[test]
    fn round_sig_figs_3_negative() {
        assert_eq!(round_sig_figs_3(-3.456), -3.46);
    }

    #[test]
    fn importance_serializes_rounded() {
        let summary = LoopDominanceSummary {
            loop_id: "L1".into(),
            name: None,
            polarity: "positive".into(),
            variables: vec![],
            importance: vec![2.449, 0.0, 0.000004781, 25.189],
        };
        let json = serde_json::to_value(&summary).unwrap();
        let arr = json["importance"].as_array().unwrap();
        assert_eq!(arr[0].as_f64().unwrap(), 2.45);
        assert_eq!(arr[1].as_f64().unwrap(), 0.0);
        assert_eq!(arr[2].as_f64().unwrap(), 4.78e-6);
        assert_eq!(arr[3].as_f64().unwrap(), 25.2);
    }

    #[test]
    fn importance_exact_values_unchanged() {
        let summary = LoopDominanceSummary {
            loop_id: "L2".into(),
            name: None,
            polarity: "negative".into(),
            variables: vec![],
            importance: vec![1.0, 100.0, 0.5],
        };
        let json = serde_json::to_value(&summary).unwrap();
        let arr = json["importance"].as_array().unwrap();
        assert_eq!(arr[0].as_f64().unwrap(), 1.0);
        assert_eq!(arr[1].as_f64().unwrap(), 100.0);
        assert_eq!(arr[2].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn error_output_serializes_camel_case() {
        let err = ErrorOutput {
            code: "unknown_dependency".to_string(),
            message: "error in model 'main' variable 'x': unknown_dependency".to_string(),
            model_name: Some("main".to_string()),
            variable_name: Some("x".to_string()),
            kind: "variable".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "unknown_dependency");
        assert_eq!(json["modelName"], "main");
        assert_eq!(json["variableName"], "x");
        assert_eq!(json["kind"], "variable");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .contains("unknown_dependency")
        );
    }

    #[test]
    fn error_output_skips_none_fields() {
        let err = ErrorOutput {
            code: "not_simulatable".to_string(),
            message: "assembly error".to_string(),
            model_name: None,
            variable_name: None,
            kind: "simulation".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert!(
            json.get("modelName").is_none(),
            "None modelName must be omitted"
        );
        assert!(
            json.get("variableName").is_none(),
            "None variableName must be omitted"
        );
        assert_eq!(json["code"], "not_simulatable");
        assert_eq!(json["kind"], "simulation");
    }

    #[test]
    fn error_output_from_formatted_error() {
        use simlin_engine::common::ErrorCode;
        use simlin_engine::errors::{FormattedError, FormattedErrorKind};

        let fe = FormattedError {
            code: ErrorCode::UnknownDependency,
            message: Some("error in model 'main' variable 'bad': unknown_dependency".to_string()),
            model_name: Some("main".to_string()),
            variable_name: Some("bad".to_string()),
            start_offset: 4,
            end_offset: 9,
            kind: FormattedErrorKind::Variable,
            unit_error_kind: None,
        };
        let output = ErrorOutput::from(&fe);
        assert_eq!(output.code, "unknown_dependency");
        assert_eq!(output.model_name.as_deref(), Some("main"));
        assert_eq!(output.variable_name.as_deref(), Some("bad"));
        assert_eq!(output.kind, "variable");
    }

    /// Verifies that `ErrorOutput::from` produces the same snake_case code
    /// strings as `ErrorCode`'s `Display` impl, which is the authoritative
    /// source shared with pysimlin (via libsimlin's `SimlinErrorCode`).
    ///
    /// Both MCP and pysimlin derive their error codes from `ErrorCode`.  MCP
    /// uses `Display` directly; pysimlin maps through `SimlinErrorCode` integer
    /// values with matching semantics.  This test locks down the string
    /// representation for the error codes most commonly encountered during
    /// model editing, ensuring the MCP `code` field stays aligned.
    #[test]
    fn error_code_strings_align_with_pysimlin() {
        use simlin_engine::common::ErrorCode;
        use simlin_engine::errors::{FormattedError, FormattedErrorKind};

        let cases: Vec<(ErrorCode, &str)> = vec![
            (ErrorCode::NoError, "no_error"),
            (ErrorCode::DoesNotExist, "does_not_exist"),
            (ErrorCode::InvalidToken, "invalid_token"),
            (ErrorCode::UnrecognizedEof, "unrecognized_eof"),
            (ErrorCode::UnrecognizedToken, "unrecognized_token"),
            (ErrorCode::ExtraToken, "extra_token"),
            (ErrorCode::UnknownBuiltin, "unknown_builtin"),
            (ErrorCode::BadBuiltinArgs, "bad_builtin_args"),
            (ErrorCode::EmptyEquation, "empty_equation"),
            (ErrorCode::NotSimulatable, "not_simulatable"),
            (ErrorCode::CircularDependency, "circular_dependency"),
            (ErrorCode::DuplicateVariable, "duplicate_variable"),
            (ErrorCode::UnknownDependency, "unknown_dependency"),
            (ErrorCode::VariablesHaveErrors, "variables_have_errors"),
            (ErrorCode::UnitMismatch, "unit_mismatch"),
            (ErrorCode::Generic, "generic"),
        ];

        for (code, expected_str) in &cases {
            // Verify Display impl produces the expected snake_case string
            assert_eq!(
                code.to_string(),
                *expected_str,
                "ErrorCode::{code:?} Display mismatch"
            );

            // Verify ErrorOutput::from uses Display for the code field
            let fe = FormattedError {
                code: *code,
                message: None,
                model_name: None,
                variable_name: None,
                start_offset: 0,
                end_offset: 0,
                kind: FormattedErrorKind::Variable,
                unit_error_kind: None,
            };
            let output = ErrorOutput::from(&fe);
            assert_eq!(
                output.code, *expected_str,
                "ErrorOutput.code for {code:?} should match Display"
            );
        }
    }
}
