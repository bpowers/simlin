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
use simlin_engine::{datamodel, json as ejson};

/// Identifies how a model file was parsed so write-back can use the same
/// format.  `Xmile` covers `.stmx`, `.xmile`, and `.xml`; `Mdl` is a Vensim
/// `.mdl` file, read by the engine's MDL parser and written back in place by
/// its MDL writer (`simlin_engine::to_mdl_with_warnings`); the JSON variants
/// are distinguished by content rather than extension (`models` vs
/// `variables` at the top level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Xmile,
    Mdl,
    NativeJson,
    SdaiJson,
}

/// Convert the MDL writer's lossiness warnings into the wire `ErrorOutput`
/// shape so they ride the same `warnings` field as the engine's other
/// non-fatal diagnostics.
///
/// An `ExportWarning` carries only a message naming the affected variable,
/// dimension, or group; there is no engine `ErrorCode` for lossiness, so
/// each rides the `generic` code (the same choice libsimlin makes for
/// `simlin_project_serialize_mdl`, so pysimlin and MCP report the same
/// thing).  The message is prefixed with `MDL export:` so an agent can tell a
/// degraded-on-disk construct from a model diagnostic.  The warning is scoped
/// to the project's single non-macro model, which is the only model an MDL
/// file can hold.
pub fn mdl_export_warnings_to_outputs(
    project: &datamodel::Project,
    warnings: &[simlin_engine::mdl::ExportWarning],
) -> Vec<ErrorOutput> {
    warnings
        .iter()
        .map(|w| mdl_export_output(project, &w.message))
        .collect()
}

/// The wire shape of an MDL writer *hard error* (a project Vensim structurally
/// cannot hold: more than one non-macro model, an ordinary Module variable).
/// Same code/kind/prefix as the warnings so a client renders both channels
/// uniformly; callers put it in a `Validation` error so the edit is rejected
/// with the violated invariant named, rather than an opaque write failure.
pub fn mdl_export_error_to_output(
    project: &datamodel::Project,
    err: &simlin_engine::Error,
) -> ErrorOutput {
    let detail = err.details.as_deref().unwrap_or("export failed");
    mdl_export_output(project, strip_mdl_export_lead_in(detail))
}

/// The engine's hard-error messages sometimes open with their own
/// "MDL export ..." lead-in; `mdl_export_output` adds the wire prefix, so
/// drop the engine's to avoid "MDL export: MDL export cannot ...".
fn strip_mdl_export_lead_in(detail: &str) -> &str {
    detail
        .strip_prefix("MDL export: ")
        .or_else(|| detail.strip_prefix("MDL export "))
        .unwrap_or(detail)
}

fn mdl_export_output(project: &datamodel::Project, message: &str) -> ErrorOutput {
    let model_name = project
        .models
        .iter()
        .find(|m| m.macro_spec.is_none())
        .map(|m| m.name.clone());
    ErrorOutput {
        code: simlin_engine::common::ErrorCode::Generic.to_string(),
        message: format!("MDL export: {message}"),
        model_name,
        variable_name: None,
        kind: "model".to_string(),
    }
}

/// Dry-run the on-disk writer for `format` without producing bytes: the
/// lossiness warnings a save would report, or the `Validation` error a save
/// would fail with. Only the MDL writer has either today; every other format
/// is lossless and returns an empty list.
///
/// `edit_model` calls this on a dry run so an agent can preview what a real
/// save would degrade, and before a real save so a structurally
/// unrepresentable project is rejected up front rather than after the
/// backing store has already merged it.
pub fn preflight_export(
    project: &datamodel::Project,
    format: SourceFormat,
) -> Result<Vec<ErrorOutput>, crate::errors::AccessError> {
    match format {
        SourceFormat::Mdl => match simlin_engine::to_mdl_with_warnings(project) {
            Ok((_text, warnings)) => Ok(mdl_export_warnings_to_outputs(project, &warnings)),
            Err(e) => Err(crate::errors::AccessError::Validation {
                errors: vec![mdl_export_error_to_output(project, &e)],
            }),
        },
        SourceFormat::Xmile | SourceFormat::NativeJson | SourceFormat::SdaiJson => Ok(Vec::new()),
    }
}

/// Sim-spec defaults used by [`build_empty_project`].
///
/// Matches the design plan's Phase 8 Note 5: an empty project gets a
/// sensible end-time, a small dt for accuracy, save-step that aligns with
/// dt boundaries, and the most universally accepted integrator (Euler).
/// Callers that need different defaults pass a custom `SimSpecs` to
/// `build_empty_project_with_specs`.
fn default_empty_sim_specs() -> ejson::SimSpecs {
    ejson::SimSpecs {
        start_time: 0.0,
        end_time: 100.0,
        dt: "0.25".to_string(),
        save_step: 1.0,
        method: "euler".to_string(),
        time_units: String::new(),
    }
}

/// Build a minimal valid `datamodel::Project` with default sim-specs and
/// one empty model named `main`.
///
/// Shared between the MCP `create_model` tool and the HTTP
/// `POST /api/projects/new` endpoint so both paths produce byte-identical
/// files when called with default inputs.  See `simlin-serve`'s parity
/// test for the byte-for-byte verification.
pub fn build_empty_project() -> datamodel::Project {
    build_empty_project_with_specs(default_empty_sim_specs())
}

/// Variant of [`build_empty_project`] that accepts a caller-supplied
/// `SimSpecs` so the MCP `create_model` tool can honour an
/// `sim_specs` override on the input without reimplementing the rest of
/// the project shape.  Default callers go through `build_empty_project`.
pub fn build_empty_project_with_specs(sim_specs: ejson::SimSpecs) -> datamodel::Project {
    let json_models = vec![ejson::Model {
        name: "main".to_string(),
        stocks: vec![],
        flows: vec![],
        auxiliaries: vec![],
        modules: vec![],
        sim_specs: None,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }];

    let json_project = ejson::Project {
        name: String::new(),
        sim_specs,
        models: json_models,
        dimensions: vec![],
        units: vec![],
        source: None,
    };

    json_project.into()
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

/// Serializes a single float rounded to 3 significant figures, matching
/// `serialize_importance`'s per-element rounding for scalar fields.
fn serialize_sig_figs_3<S: serde::Serializer>(
    value: &f64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(round_sig_figs_3(*value))
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
    /// Per-timestep SIGNED partition-relative loop score in `[-1, 1]` (the
    /// engine `LoopSummary.importance`): the loop's share of its cycle
    /// partition's total |loop score|, sign preserved.  Comparable across
    /// loops/partitions, so a larger `|importance|` means a more dominant loop.
    #[serde(serialize_with = "serialize_importance")]
    pub importance: Vec<f64>,
    /// Polarity-confidence ratio in `[0.0, 1.0]` (GH #495): `1.0` for a clean
    /// reinforcing/balancing loop, below 1.0 for a mixed-sign
    /// `mostlyReinforcing`/`mostlyBalancing` loop, `0.0` for `undetermined`.
    /// Rounded to 3 significant figures like `importance` to keep MCP output
    /// compact; additive so existing clients see the new field but the prior
    /// shape is unchanged.
    #[serde(serialize_with = "serialize_sig_figs_3")]
    pub polarity_confidence: f64,
    /// RESULT-SCOPED index into the analyze output's `partitions` list naming
    /// this loop's cycle partition, or absent (`None`) for a loop with no
    /// parent-level partition.  Indices are dense and in first-appearance order
    /// over `loopDominance`; they are NOT stable across edits -- key on a
    /// `PartitionOutput.stocks` set for a durable identity.  Additive and
    /// elided when absent so the prior wire shape is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<usize>,
}

impl From<simlin_engine::analysis::LoopSummary> for LoopDominanceSummary {
    fn from(ls: simlin_engine::analysis::LoopSummary) -> Self {
        Self {
            loop_id: ls.loop_id,
            name: ls.name,
            polarity: ls.polarity,
            variables: ls.variables,
            importance: ls.importance,
            polarity_confidence: ls.polarity_confidence,
            partition: ls.partition,
        }
    }
}

/// One cycle partition referenced by an analyze result's loops: a group of
/// stocks connected by feedback, within which relative loop scores are
/// comparable.  Mirrors the engine `DiscoveredPartition` / pysimlin
/// `Partition`; lets a client group loops partition-by-partition.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartitionOutput {
    /// The partition's stock names (element-level for arrayed models, e.g.
    /// `population[nyc]`), sorted lexicographically.  This SET is the durable
    /// per-result identity.  Since GH #746 the exhaustive (`Model.loops`)
    /// surface partitions at the same ELEMENT granularity, so the stock sets
    /// agree across the two surfaces for scalar AND arrayed models (indices
    /// remain result-scoped and may differ).
    pub stocks: Vec<String>,
    /// Number of loops in the returned `loopDominance` list that belong to
    /// this partition.
    pub loop_count: usize,
}

impl From<&simlin_engine::ltm_finding::DiscoveredPartition> for PartitionOutput {
    fn from(p: &simlin_engine::ltm_finding::DiscoveredPartition) -> Self {
        Self {
            stocks: p.stocks.clone(),
            loop_count: p.loop_count,
        }
    }
}

/// A time interval during which specific loops dominate model behavior.
///
/// Dominance is computed WITHIN a cycle partition (GH #998): a loop's
/// importance is its share of its own partition's total, so cross-partition
/// ranking is not well-defined.  `partition` says which partition the period
/// describes (indexing the result's `partitions` list, the same space as
/// `LoopDominanceSummary::partition`); the output carries one period
/// timeline per partition, most-competitive partition first.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DominantPeriodOutput {
    pub dominant_loops: Vec<String>,
    pub start_time: f64,
    pub end_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<usize>,
}

impl From<simlin_engine::ltm_dominance::DominantPeriod> for DominantPeriodOutput {
    fn from(dp: simlin_engine::ltm_dominance::DominantPeriod) -> Self {
        Self {
            dominant_loops: dp.dominant_loops,
            start_time: dp.start,
            end_time: dp.end,
            partition: dp.partition,
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
    fn build_empty_project_has_one_model_named_main_with_no_variables() {
        let project = build_empty_project();
        assert_eq!(
            project.models.len(),
            1,
            "empty project must contain exactly one model"
        );
        let main = &project.models[0];
        assert_eq!(main.name.as_str(), "main");
        assert!(main.variables.is_empty(), "main must have no variables");
        assert!(main.views.is_empty(), "main must have no views");
    }

    #[test]
    fn build_empty_project_uses_canonical_default_sim_specs() {
        // Locking the defaults so the parity test (HTTP create vs MCP
        // create_model) keeps producing byte-identical files.
        let project = build_empty_project();
        let specs = &project.sim_specs;
        assert_eq!(specs.start, 0.0);
        assert_eq!(specs.stop, 100.0);
        assert_eq!(specs.dt, simlin_engine::datamodel::Dt::Dt(0.25));
        assert_eq!(specs.save_step, Some(simlin_engine::datamodel::Dt::Dt(1.0)));
        assert_eq!(specs.sim_method, simlin_engine::datamodel::SimMethod::Euler);
    }

    #[test]
    fn build_empty_project_with_specs_carries_caller_overrides() {
        let custom = ejson::SimSpecs {
            start_time: 5.0,
            end_time: 50.0,
            dt: "0.5".to_string(),
            save_step: 2.0,
            method: "rk4".to_string(),
            time_units: "weeks".to_string(),
        };
        let project = build_empty_project_with_specs(custom);
        assert_eq!(project.sim_specs.start, 5.0);
        assert_eq!(project.sim_specs.stop, 50.0);
        assert_eq!(project.sim_specs.dt, simlin_engine::datamodel::Dt::Dt(0.5));
        assert_eq!(
            project.sim_specs.sim_method,
            simlin_engine::datamodel::SimMethod::RungeKutta4
        );
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("weeks"));
    }

    #[test]
    fn round_sig_figs_3_covers_each_magnitude_and_sign() {
        // (input, expected) across the cases the scientific-notation
        // round trip has to get right: the zero short-circuit, a value
        // below 1 (negative exponent), one above 1, a negative, and
        // values already at or below 3 significant figures, which must
        // come back bit-identical rather than being perturbed.
        let cases = [
            (0.0, 0.0),
            (2.449215777949112, 2.45),
            (0.000004781283, 4.78e-6),
            (25.189, 25.2),
            (-3.456, -3.46),
            (1.0, 1.0),
            (100.0, 100.0),
            (0.5, 0.5),
        ];
        for (input, expected) in cases {
            assert_eq!(
                round_sig_figs_3(input),
                expected,
                "round_sig_figs_3({input})"
            );
        }
    }

    #[test]
    fn importance_serializes_rounded() {
        let summary = LoopDominanceSummary {
            loop_id: "L1".into(),
            name: None,
            polarity: "positive".into(),
            variables: vec![],
            importance: vec![2.449, 0.0, 0.000004781, 25.189],
            polarity_confidence: 1.0,
            partition: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        let arr = json["importance"].as_array().unwrap();
        assert_eq!(arr[0].as_f64().unwrap(), 2.45);
        assert_eq!(arr[1].as_f64().unwrap(), 0.0);
        assert_eq!(arr[2].as_f64().unwrap(), 4.78e-6);
        assert_eq!(arr[3].as_f64().unwrap(), 25.2);
    }

    #[test]
    fn polarity_confidence_serializes_camel_case_rounded() {
        // The mixed-sign Rux/Bux confidence (GH #495) rides on the wire as a
        // camelCase, 3-sig-fig field alongside importance.
        let summary = LoopDominanceSummary {
            loop_id: "L3".into(),
            name: None,
            polarity: "mostly_reinforcing".into(),
            variables: vec![],
            importance: vec![0.5],
            polarity_confidence: 0.993871,
            partition: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["polarityConfidence"].as_f64().unwrap(), 0.994);
        assert_eq!(json["polarity"].as_str().unwrap(), "mostly_reinforcing");
    }

    /// The MDL lossiness channel rides the same `ErrorOutput` shape as every
    /// other non-fatal diagnostic: `generic` code (no engine code exists for
    /// lossiness), model-scoped to the single non-macro model, and a message
    /// an agent can attribute to the on-disk export rather than the model.
    #[test]
    fn mdl_export_warnings_map_to_generic_model_scoped_outputs() {
        let mut project = build_empty_project();
        project.models[0].name = "cooling".to_string();
        let warnings = vec![
            simlin_engine::mdl::ExportWarning {
                message: "flow 'heat loss' is non-negative; Vensim cannot express this".to_string(),
            },
            simlin_engine::mdl::ExportWarning {
                message: "graphical function for 'demand' uses discrete interpolation".to_string(),
            },
        ];
        let outputs = mdl_export_warnings_to_outputs(&project, &warnings);
        assert_eq!(outputs.len(), 2);
        for (out, w) in outputs.iter().zip(&warnings) {
            assert_eq!(out.code, "generic");
            assert_eq!(out.kind, "model");
            assert_eq!(out.model_name.as_deref(), Some("cooling"));
            assert_eq!(out.variable_name, None);
            assert_eq!(out.message, format!("MDL export: {}", w.message));
        }
        assert!(mdl_export_warnings_to_outputs(&project, &[]).is_empty());
    }

    /// The engine's own "MDL export ..." lead-in is stripped so the wire
    /// prefix is not doubled; a message without one passes through, and a
    /// missing detail gets a generic body.
    #[test]
    fn mdl_export_error_to_output_does_not_double_the_prefix() {
        use simlin_engine::common::{ErrorCode, ErrorKind};
        let project = build_empty_project();
        let cases = [
            (
                Some("MDL export cannot faithfully reconstruct 1 macro"),
                "MDL export: cannot faithfully reconstruct 1 macro",
            ),
            (Some("MDL export: something"), "MDL export: something"),
            (
                Some("MDL format supports only a single model"),
                "MDL export: MDL format supports only a single model",
            ),
            (None, "MDL export: export failed"),
        ];
        for (detail, expected) in cases {
            let err = simlin_engine::Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                detail.map(str::to_string),
            );
            assert_eq!(mdl_export_error_to_output(&project, &err).message, expected);
        }
    }

    /// `preflight_export` is a four-way dispatch on `SourceFormat`; the three
    /// lossless arms return nothing, and the `Mdl` arm forwards the writer's
    /// two channels: warnings for a degraded construct, and a `Validation`
    /// error (same generic/model/`MDL export:` shape) for a project Vensim
    /// structurally cannot hold.
    #[test]
    fn preflight_export_covers_every_format_and_both_mdl_channels() {
        let clean = build_empty_project();
        for format in [
            SourceFormat::Xmile,
            SourceFormat::NativeJson,
            SourceFormat::SdaiJson,
            SourceFormat::Mdl,
        ] {
            assert_eq!(
                preflight_export(&clean, format).expect("clean project"),
                Vec::new(),
                "{format:?}"
            );
        }

        let mut lossy = build_empty_project();
        lossy.models[0].loop_metadata.push(datamodel::LoopMetadata {
            uids: vec![1],
            deleted: false,
            name: "Growth".to_string(),
            description: String::new(),
        });
        let warnings = preflight_export(&lossy, SourceFormat::Mdl).expect("lossy still exports");
        assert!(
            warnings
                .iter()
                .any(|w| w.message.starts_with("MDL export:") && w.message.contains("Growth")),
            "{warnings:?}"
        );

        let mut two_models = build_empty_project();
        let mut second = two_models.models[0].clone();
        second.name = "second".to_string();
        two_models.models.push(second);
        match preflight_export(&two_models, SourceFormat::Mdl) {
            Err(crate::errors::AccessError::Validation { errors }) => {
                assert_eq!(errors.len(), 1);
                assert_eq!(errors[0].code, "generic");
                assert_eq!(errors[0].kind, "model");
                assert!(errors[0].message.starts_with("MDL export:"), "{errors:?}");
                assert!(errors[0].message.contains("single model"), "{errors:?}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
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
        use simlin_engine::db::DiagnosticSeverity;
        use simlin_engine::errors::{FormattedError, FormattedErrorKind};

        let fe = FormattedError {
            code: ErrorCode::UnknownDependency,
            message: Some("error in model 'main' variable 'bad': unknown_dependency".to_string()),
            model_name: Some("main".to_string()),
            variable_name: Some("bad".to_string()),
            start_offset: 4,
            end_offset: 9,
            kind: FormattedErrorKind::Variable,
            severity: DiagnosticSeverity::Error,
            unit_error_kind: None,
            details: None,
        };
        let output = ErrorOutput::from(&fe);
        assert_eq!(output.code, "unknown_dependency");
        assert_eq!(output.model_name.as_deref(), Some("main"));
        assert_eq!(output.variable_name.as_deref(), Some("bad"));
        assert_eq!(output.kind, "variable");
    }

    /// Pins the snake_case strings `ErrorOutput::from` puts on the wire, and
    /// that it sources them from `ErrorCode`'s `Display` impl rather than a
    /// second table of its own.
    ///
    /// The MCP `code` field is the only thing asserted here. pysimlin reaches
    /// the same codes by a different route (libsimlin's `SimlinErrorCode`
    /// integer values), and the two surfaces are believed to agree on the
    /// commonly-encountered codes below -- but nothing in this test measures
    /// pysimlin, so treat that as background, not as a verified claim. The
    /// codes outside `SimlinErrorCode` are a documented divergence in any
    /// case: they collapse to `Generic` at the C boundary while MCP keeps
    /// reporting the precise string.
    #[test]
    fn error_code_strings_are_the_display_impl() {
        use simlin_engine::common::ErrorCode;
        use simlin_engine::db::DiagnosticSeverity;
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
                severity: DiagnosticSeverity::Error,
                unit_error_kind: None,
                details: None,
            };
            let output = ErrorOutput::from(&fe);
            assert_eq!(
                output.code, *expected_str,
                "ErrorOutput.code for {code:?} should match Display"
            );
        }
    }
}
