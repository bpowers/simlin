// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Async `ReadModel` library function.
//!
//! Loads a project via [`ProjectAccess`] and returns a snapshot enriched
//! with the engine's loop-dominance analysis.  All I/O is delegated to
//! the access impl so the same body runs against either a stateless
//! filesystem-backed impl or the registry-backed impl introduced in
//! Phase 6.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json;

use crate::access::ProjectAccess;
use crate::errors::AccessError;
use crate::open::resolve_model_name;
use crate::types::{DominantPeriodOutput, ErrorOutput, LoopDominanceSummary, PartitionOutput};

/// Input for the `ReadModel` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadModelInput {
    /// Absolute or relative path to the model file (XMILE .stmx/.xmile,
    /// Vensim .mdl, or Simlin .simlin JSON).
    pub project_path: String,

    /// Name of the model within the project to analyse.
    /// Defaults to "main" when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

/// Output from the `ReadModel` tool.
///
/// `errors` is `Vec` rather than `Option<Vec>` so the field's presence in
/// memory is easy to test; the `skip_serializing_if = "Vec::is_empty"`
/// attribute keeps the wire shape identical to the pre-refactor binary.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadModelOutput {
    pub model: json::Model,
    pub time: Vec<f64>,
    pub loop_dominance: Vec<LoopDominanceSummary>,
    /// The cycle partitions referenced by `loopDominance` (each summary's
    /// `partition` indexes this list).  Elided when empty to preserve the
    /// stable wire shape; the stock SET of each entry is the durable identity
    /// matching the exhaustive (`Model.loops`) surface.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<PartitionOutput>,
    pub dominant_loops_by_period: Vec<DominantPeriodOutput>,
    /// True when discovery's cross-element-through-aggregate loop recovery hit
    /// its budget, so `loopDominance` may be missing some cross-agg reducer
    /// loops. A result-level structural-completeness signal (not per-loop);
    /// elided when false to preserve the stable wire shape.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agg_recovery_truncated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorOutput>,
}

/// Read a model and return its JSON snapshot.
///
/// Reads bytes via `access.open(...)`, parses, runs diagnostics, and
/// produces the same wire-shape output the existing binary emits.  When
/// the model has no errors, the `errors` field is empty (and serde
/// elides it from JSON output via `skip_serializing_if`).  Diagnostics
/// are scoped to the requested model so errors in sibling models in a
/// multi-model project do not bleed through.
pub async fn read_model<A: ProjectAccess>(
    access: &A,
    input: ReadModelInput,
) -> Result<ReadModelOutput, AccessError> {
    let path = Path::new(&input.project_path);
    let opened = access.open(path).await?;
    let project = opened.project;

    let requested_name = input.model_name.as_deref().unwrap_or("main");
    let model_name = resolve_model_name(&project, requested_name);

    let mut db = simlin_engine::db::SimlinDb::default();
    let sync = simlin_engine::db::sync_from_datamodel(&db, &project);
    let source_project = sync.project;

    let diagnostics = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let errors: Vec<ErrorOutput> = {
        let has_errors = diagnostics
            .iter()
            .any(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Error));
        if !has_errors {
            vec![]
        } else {
            simlin_engine::errors::collect_formatted_errors(
                diagnostics
                    .iter()
                    .filter(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Error)),
                &project,
            )
            .errors
            .iter()
            // Errors from sibling models in a multi-model project would confuse
            // a client reading a clean model; project-level errors (no model
            // name) still surface.
            .filter(|e| e.model_name.as_ref().is_none_or(|name| name == model_name))
            .map(ErrorOutput::from)
            .collect()
        }
    };

    // No discovery budget here: ReadModel runs on user-opened models that are
    // already known-tractable for the MCP surface. The opt-in budgeted path is
    // pysimlin's `Model.analyze(timeout=...)`.
    let analysis =
        simlin_engine::analysis::analyze_model(&project, &mut db, source_project, model_name, None)
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("analysis failed: {e}")))?;

    let agg_recovery_truncated = analysis.agg_recovery_truncated;
    let partitions: Vec<PartitionOutput> = analysis.partitions.iter().map(Into::into).collect();
    let loop_dominance: Vec<LoopDominanceSummary> = analysis
        .loop_dominance
        .into_iter()
        .map(Into::into)
        .collect();

    let dominant_loops_by_period: Vec<DominantPeriodOutput> = analysis
        .dominant_loops_by_period
        .into_iter()
        .map(Into::into)
        .collect();

    Ok(ReadModelOutput {
        model: analysis.model,
        time: analysis.time,
        loop_dominance,
        partitions,
        dominant_loops_by_period,
        agg_recovery_truncated,
        errors,
    })
}
