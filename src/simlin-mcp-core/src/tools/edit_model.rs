// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Async `EditModel` library function with diagnostic-gated writes.
//!
//! Curated input types here deliberately exclude the engine-internal
//! `uid`, `compat`, and `aiState` fields so an LLM never has to author
//! bookkeeping data.  After patch application a salsa-driven diagnostic
//! pass ensures the edit does not introduce *new* errors (existing
//! errors are tolerated so models can be repaired incrementally); a
//! gate failure surfaces as `AccessError::Validation { errors }` with
//! the same `ErrorOutput` shape the wire format already uses.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use crate::access::ProjectAccess;
use crate::errors::AccessError;
use crate::open::resolve_model_name;
use crate::types::{DominantPeriodOutput, ErrorOutput, LoopDominanceSummary, PartitionOutput};

// ── Curated input types ───────────────────────────────────────────────
//
// These types expose only the fields meaningful to an LLM building a
// model.  `uid`, `compat`, and `aiState` are intentionally excluded.

/// An element equation for one subscript element of an arrayed variable.
/// Excludes `compat` (internal compatibility field).
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ElementEquationInput {
    /// Subscript label for this element.
    pub subscript: String,
    /// Equation for this element.
    pub equation: String,
    /// Optional graphical (table) function for this element.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
}

/// Equation for an arrayed (subscripted) variable.
/// Excludes `compat` (internal compatibility field).
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrayedEquationInput {
    /// Dimension names for this array.
    pub dimensions: Vec<String>,
    /// Apply-to-all equation (when the same equation applies to every element).
    #[serde(default)]
    pub equation: Option<String>,
    /// Per-element equations (for element-wise overrides).
    #[serde(default)]
    pub elements: Option<Vec<ElementEquationInput>>,
}

/// Create or update a stock (accumulator) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertStockInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Initial value equation for the stock.
    pub initial_equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Names of flows that flow into this stock.
    #[serde(default)]
    pub inflows: Option<Vec<String>>,
    /// Names of flows that flow out of this stock.
    #[serde(default)]
    pub outflows: Option<Vec<String>>,
    /// Equation for arrayed (subscripted) stocks.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Create or update a flow (rate) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertFlowInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Rate equation for this flow.
    pub equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Optional graphical (table) function.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
    /// Equation for arrayed (subscripted) flows.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Create or update an auxiliary (constant or computed) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertAuxiliaryInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Equation for this auxiliary variable.
    pub equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Optional graphical (table) function.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
    /// Equation for arrayed (subscripted) auxiliaries.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Remove a variable from the model by name.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoveVariableInput {
    /// Name of the variable to remove.
    pub name: String,
}

/// Assign a human-readable name to a feedback loop identified by its
/// participating variables.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetLoopNameInput {
    /// Variable names that form the loop (order does not matter).
    pub variables: Vec<String>,
    /// Human-readable name for the loop.
    pub name: String,
    /// Optional description of the loop's behavior.
    pub description: Option<String>,
}

/// An edit operation on a model.
///
/// Serde's default externally-tagged representation produces JSON like:
/// `{ "upsertStock": { "name": "population", ... } }`
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EditOperation {
    UpsertStock(UpsertStockInput),
    UpsertFlow(UpsertFlowInput),
    UpsertAuxiliary(UpsertAuxiliaryInput),
    RemoveVariable(RemoveVariableInput),
    SetLoopName(SetLoopNameInput),
}

/// Input for the `EditModel` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditModelInput {
    /// Absolute or relative path to the model file.
    pub project_path: String,

    /// Name of the model within the project to edit. Defaults to "main".
    #[serde(default)]
    pub model_name: Option<String>,

    /// If true, validate and preview changes without writing to disk.
    #[serde(default)]
    pub dry_run: Option<bool>,

    /// Optional simulation spec changes to apply before variable operations.
    #[serde(default)]
    pub sim_specs: Option<ejson::SimSpecs>,

    /// Variable operations to apply in order.
    #[serde(default)]
    pub operations: Option<Vec<EditOperation>>,
}

/// Output from the `EditModel` tool — matches `ReadModel` shape plus
/// the `dry_run` flag.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditModelOutput {
    /// Path where the model was written -- always the same as the input
    /// path, regardless of source format.
    pub project_path: String,
    pub model: ejson::Model,
    pub time: Vec<f64>,
    pub loop_dominance: Vec<LoopDominanceSummary>,
    /// The cycle partitions referenced by `loopDominance` (each summary's
    /// `partition` indexes this list).  Elided when empty to preserve the
    /// stable wire shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<PartitionOutput>,
    pub dominant_loops_by_period: Vec<DominantPeriodOutput>,
    /// True when discovery's cross-element-through-aggregate loop recovery hit
    /// its budget, so `loopDominance` may be missing some cross-agg reducer
    /// loops. A result-level structural-completeness signal (not per-loop);
    /// elided when false to preserve the stable wire shape.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agg_recovery_truncated: bool,
    /// Non-fatal diagnostics scoped to the edited model (the LTM auto-flip
    /// advisory and synthetic-fragment compile-failure warnings, GH #662).
    /// Empty (and elided from JSON) when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ErrorOutput>,
    /// `Some(message)` when the just-edited model could not be compiled for
    /// LTM loop analysis (so `loop_dominance` is empty because of a failure,
    /// not an absence of loops); see `ReadModelOutput::analysis_error` and
    /// GH #660.  Elided from the wire shape when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_error: Option<String>,
    pub dry_run: bool,
}

/// Apply curated edit operations to a project.
///
/// Reads the project via `access.open(...)`, applies the patch, runs a
/// diagnostic pass, and writes the result back via `access.save(...)`
/// when no new errors were introduced.  Existing errors are tolerated
/// so an LLM can repair a broken model incrementally; an edit is
/// rejected only when it introduces a (code, variable_name) pair that
/// was not present before.  Failures during the validation pass surface
/// as `AccessError::Validation { errors: Vec<ErrorOutput> }` so the
/// rmcp tool layer can serialise the same wire shape `ReadModel`
/// already emits.
pub async fn edit_model<A: ProjectAccess>(
    access: &A,
    input: EditModelInput,
) -> Result<EditModelOutput, AccessError> {
    let path = Path::new(&input.project_path);

    // Vensim .mdl files are read-only here so an LLM gets a clear pointer
    // to CreateModel rather than a generic write failure deep in the
    // engine.  The Phase 6 RegistryAccess will revisit this when it gains
    // sidecar support.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext == "mdl" {
        return Err(AccessError::ParseError(anyhow::anyhow!(
            "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, \
             then CreateModel to start a new .sd.json file you can edit."
        )));
    }

    let opened = access.open(path).await?;
    let mut project = opened.project;
    let source_format = opened.source_format;
    let expected_version = opened.version;

    let requested_name = input.model_name.as_deref().unwrap_or("main");
    let model_name = resolve_model_name(&project, requested_name).to_string();
    let dry_run = input.dry_run.unwrap_or(false);

    // SetLoopName only writes loop metadata -- it doesn't add, remove, or
    // rename any variables, so it must not trigger diagram regeneration.
    let has_variable_ops = input.operations.as_ref().is_some_and(|ops| {
        ops.iter()
            .any(|op| !matches!(op, EditOperation::SetLoopName(_)))
    });

    // Collect pre-edit error signatures for the target model so we can
    // detect NEW errors after the edit. Comparing on (error_code,
    // variable_name) rather than count means edits that fix one error
    // while introducing a different one are still rejected even though
    // the count stayed the same.
    //
    // EditModel always runs LTM loop analysis (via `analyze_model`), so both
    // diagnostic passes collect with LTM transiently enabled (GH #662) -- the
    // same shared `LtmEnabledGuard` libsimlin uses (GH #466). Enabling LTM on
    // the pre- AND post-edit passes keeps the new-error delta symmetric: the
    // LTM-only diagnostics (advisory Warnings; the GH #486 non-Euler rejection
    // rides the assemble path, not this accumulator) are computed the same way
    // on both sides, so they can never spuriously read as a "new error".
    let pre_edit_error_keys: std::collections::HashSet<_> = {
        let mut pre_db = simlin_engine::db::SimlinDb::default();
        let pre_sync = simlin_engine::db::sync_from_datamodel(&pre_db, &project);
        let pre_source_project = pre_sync.project;
        let all_diags = {
            let guard =
                simlin_engine::db::LtmEnabledGuard::enable(&mut pre_db, pre_source_project, true);
            simlin_engine::db::collect_all_diagnostics(guard.db(), pre_source_project)
        };
        simlin_engine::errors::collect_formatted_errors(
            all_diags
                .iter()
                .filter(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Error)),
            &project,
        )
        .errors
        .into_iter()
        .filter(|e| e.model_name.as_ref().is_none_or(|name| name == &model_name))
        .map(|e| (e.code, e.variable_name))
        .collect()
    };

    let patch = build_patch(&model_name, input.sim_specs, input.operations);
    let model_patch = patch.models.iter().find(|m| m.name == model_name).cloned();
    simlin_engine::apply_patch(&mut project, patch)
        .map_err(|e| AccessError::ParseError(anyhow::anyhow!("patch application failed: {e:?}")))?;

    if !dry_run && has_variable_ops {
        sync_diagram(&mut project, &model_name, model_patch.as_ref());
    }

    // One SimlinDb shared between the diagnostic gate and analyze_model
    // so salsa's caches are reused.
    let mut db = simlin_engine::db::SimlinDb::default();
    let sync = simlin_engine::db::sync_from_datamodel(&db, &project);
    let source_project = sync.project;

    // Collect post-edit diagnostics with LTM transiently enabled (GH #662), so
    // LTM advisory Warnings reach the caller. The guard restores `ltm_enabled`
    // to false on drop -- before the `analyze_model` call below, which runs its
    // own flag dance -- so the two passes don't interfere.
    let all_diagnostics = {
        let guard = simlin_engine::db::LtmEnabledGuard::enable(&mut db, source_project, true);
        simlin_engine::db::collect_all_diagnostics(guard.db(), source_project)
    };
    let post_formatted = simlin_engine::errors::collect_formatted_errors(
        all_diagnostics
            .iter()
            .filter(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Error)),
        &project,
    );
    let post_edit_model_errors: Vec<_> = post_formatted
        .errors
        .iter()
        .filter(|e| e.model_name.as_ref().is_none_or(|name| name == &model_name))
        .collect();

    // Model-scoped non-fatal warnings (e.g. the LTM auto-flip advisory) to
    // include in the success response (GH #662).
    let warnings: Vec<ErrorOutput> = simlin_engine::errors::collect_formatted_errors(
        all_diagnostics
            .iter()
            .filter(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Warning)),
        &project,
    )
    .errors
    .iter()
    .filter(|e| e.model_name.as_ref().is_none_or(|name| name == &model_name))
    .map(ErrorOutput::from)
    .collect();

    let has_new_errors = post_edit_model_errors
        .iter()
        .any(|e| !pre_edit_error_keys.contains(&(e.code, e.variable_name.clone())));

    if has_new_errors {
        let error_outputs: Vec<ErrorOutput> = post_edit_model_errors
            .iter()
            .map(|e| ErrorOutput::from(*e))
            .collect();
        return Err(AccessError::Validation {
            errors: error_outputs,
        });
    }

    // No discovery budget here (see the rationale in read_model.rs): EditModel
    // analyses the just-edited model for the MCP response, not a bulk run.
    let analysis = simlin_engine::analysis::analyze_model(
        &project,
        &mut db,
        source_project,
        &model_name,
        None,
    )
    .map_err(|e| AccessError::ParseError(anyhow::anyhow!("analysis failed: {e}")))?;

    if !dry_run {
        access
            .save(path, &project, source_format, Some(expected_version))
            .await?;
    }

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

    Ok(EditModelOutput {
        project_path: path.display().to_string(),
        model: analysis.model,
        time: analysis.time,
        loop_dominance,
        partitions,
        dominant_loops_by_period,
        agg_recovery_truncated,
        warnings,
        analysis_error: analysis.analysis_error,
        dry_run,
    })
}

/// Regenerate the diagram layout for the named model, replacing its views
/// in-place.  When a model patch is provided and the model already has a
/// non-empty view, uses incremental layout to preserve existing element
/// positions.  Falls back to full layout generation otherwise.
///
/// Preserves the existing zoom level when the model already has a view.
/// Layout failures are silently ignored -- a missing diagram is non-fatal
/// and the model data is still correct.
fn sync_diagram(
    project: &mut simlin_engine::datamodel::Project,
    model_name: &str,
    model_patch: Option<&simlin_engine::ModelPatch>,
) {
    let old_view = project
        .get_model(model_name)
        .and_then(|m| m.views.first())
        .map(|v| match v {
            simlin_engine::datamodel::View::StockFlow(sf) => sf,
        });

    let existing_zoom = old_view.map(|sf| sf.zoom).filter(|&z| z > 0.0);

    let new_view = if let (Some(old_sf), Some(patch)) = (old_view, model_patch) {
        if !old_sf.elements.is_empty() {
            let old_sf = old_sf.clone();
            simlin_engine::layout::incremental_layout(&old_sf, project, model_name, patch, None)
        } else {
            simlin_engine::layout::generate_best_layout(project, model_name, None)
        }
    } else {
        simlin_engine::layout::generate_best_layout(project, model_name, None)
    };

    let mut layout = match new_view {
        Ok(l) => l,
        Err(_) => return,
    };

    if let Some(zoom) = existing_zoom {
        layout.zoom = zoom;
    }

    if let Some(model) = project.get_model_mut(model_name) {
        model.views = vec![simlin_engine::datamodel::View::StockFlow(layout)];
    }
}

/// Build an engine `ProjectPatch` from the curated MCP inputs.
///
/// `simSpecs` changes become a project-level operation applied first,
/// then variable operations are added as model-level operations.
///
/// Public so `simulate` can reuse the same translation rules: any rule
/// drift between `EditModel` and `Simulate` would surface as confusing
/// "the same operation behaves differently in simulate" bugs.
pub fn build_patch(
    model_name: &str,
    sim_specs: Option<ejson::SimSpecs>,
    operations: Option<Vec<EditOperation>>,
) -> simlin_engine::ProjectPatch {
    let project_ops = sim_specs
        .map(|ss| vec![simlin_engine::ProjectOperation::SetSimSpecs(ss.into())])
        .unwrap_or_default();

    let model_ops: Vec<simlin_engine::ModelOperation> = operations
        .unwrap_or_default()
        .into_iter()
        .map(convert_operation)
        .collect();

    let models = if model_ops.is_empty() {
        vec![]
    } else {
        vec![simlin_engine::ModelPatch {
            name: model_name.to_string(),
            ops: model_ops,
        }]
    };

    simlin_engine::ProjectPatch {
        project_ops,
        models,
    }
}

/// Convert a curated `ArrayedEquationInput` to an `ejson::ArrayedEquation`,
/// filling in `compat: None` for the excluded field.
fn convert_arrayed_equation(a: ArrayedEquationInput) -> ejson::ArrayedEquation {
    // EXCEPT semantics: a default equation with per-element overrides.
    // When both are present, the default applies to unspecified elements.
    let has_except_default = if a.equation.is_some() && a.elements.is_some() {
        Some(true)
    } else {
        None
    };
    ejson::ArrayedEquation {
        dimensions: a.dimensions,
        equation: a.equation,
        compat: None,
        elements: a.elements.map(|els| {
            els.into_iter()
                .map(|el| ejson::ElementEquation {
                    subscript: el.subscript,
                    equation: el.equation,
                    compat: None,
                    graphical_function: el.graphical_function,
                })
                .collect()
        }),
        has_except_default,
    }
}

/// Convert a curated `EditOperation` to an engine `ModelOperation`.
///
/// Excluded fields (`uid`, `compat`) are filled with engine defaults
/// (0 / None).
fn convert_operation(op: EditOperation) -> simlin_engine::ModelOperation {
    match op {
        EditOperation::UpsertStock(s) => {
            let json_stock = ejson::Stock {
                uid: 0,
                name: s.name,
                initial_equation: s.initial_equation,
                units: s.units.unwrap_or_default(),
                inflows: s.inflows.unwrap_or_default(),
                outflows: s.outflows.unwrap_or_default(),
                documentation: s.documentation.unwrap_or_default(),
                arrayed_equation: s.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                non_negative: false,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertStock(json_stock.into())
        }
        EditOperation::UpsertFlow(f) => {
            let json_flow = ejson::Flow {
                uid: 0,
                name: f.name,
                equation: f.equation,
                units: f.units.unwrap_or_default(),
                graphical_function: f.graphical_function,
                documentation: f.documentation.unwrap_or_default(),
                arrayed_equation: f.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                non_negative: false,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertFlow(json_flow.into())
        }
        EditOperation::UpsertAuxiliary(a) => {
            let json_aux = ejson::Auxiliary {
                uid: 0,
                name: a.name,
                equation: a.equation,
                units: a.units.unwrap_or_default(),
                graphical_function: a.graphical_function,
                documentation: a.documentation.unwrap_or_default(),
                arrayed_equation: a.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertAux(json_aux.into())
        }
        EditOperation::RemoveVariable(r) => {
            simlin_engine::ModelOperation::DeleteVariable { ident: r.name }
        }
        EditOperation::SetLoopName(input) => simlin_engine::ModelOperation::SetLoopName {
            variables: input.variables,
            name: input.name,
            description: input.description,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_arrayed_equation_infers_except_default() {
        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: Some("default_eq".to_string()),
            elements: Some(vec![ElementEquationInput {
                subscript: "A1".to_string(),
                equation: "override_eq".to_string(),
                graphical_function: None,
            }]),
        };
        let result = convert_arrayed_equation(input);
        assert_eq!(result.has_except_default, Some(true));
    }

    #[test]
    fn convert_arrayed_equation_no_except_without_both() {
        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: Some("eq".to_string()),
            elements: None,
        };
        assert_eq!(convert_arrayed_equation(input).has_except_default, None);

        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: None,
            elements: Some(vec![ElementEquationInput {
                subscript: "A1".to_string(),
                equation: "eq".to_string(),
                graphical_function: None,
            }]),
        };
        assert_eq!(convert_arrayed_equation(input).has_except_default, None);
    }

    #[test]
    fn convert_set_loop_name_operation() {
        let op = EditOperation::SetLoopName(SetLoopNameInput {
            variables: vec!["population".into(), "births".into()],
            name: "Growth Loop".into(),
            description: Some("reinforcing growth".into()),
        });
        let model_op = convert_operation(op);
        match model_op {
            simlin_engine::ModelOperation::SetLoopName {
                variables,
                name,
                description,
            } => {
                assert_eq!(variables, vec!["population", "births"]);
                assert_eq!(name, "Growth Loop");
                assert_eq!(description.as_deref(), Some("reinforcing growth"));
            }
            _ => panic!("expected SetLoopName variant"),
        }
    }
}
