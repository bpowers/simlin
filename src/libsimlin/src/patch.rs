// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Patch types, conversion, validation, and FFI.
//!
//! JSON-based project patching: deserialize a JSON patch, convert to the
//! engine's `ProjectPatch`, apply it (with optional dry-run), and collect
//! both compile-time and static-analysis errors.

use serde::Deserialize;
use simlin_engine::common::ErrorCode;
use simlin_engine::{self as engine, Vm};
use std::ptr;

use crate::errors;
pub use crate::ffi_error::ErrorDetail as ErrorDetailData;
use crate::ffi_error::{FfiError, SimlinError};
use crate::simulation::compile_simulation;
use crate::{
    build_simlin_error, clear_out_error, require_project, store_anyhow_error, store_error,
    store_ffi_error, SimlinErrorCode, SimlinErrorKind, SimlinProject, SimlinUnitErrorKind,
};

// ── JSON serde types ───────────────────────────────────────────────────

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
enum JsonProjectOperation {
    SetSimSpecs {
        #[serde(rename = "simSpecs")]
        sim_specs: engine::json::SimSpecs,
    },
    AddModel {
        name: String,
    },
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonProjectPatch {
    #[serde(default)]
    project_ops: Vec<JsonProjectOperation>,
    #[serde(default)]
    models: Vec<JsonModelPatch>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Deserialize)]
struct JsonModelPatch {
    name: String,
    #[serde(default)]
    ops: Vec<JsonModelOperation>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
enum JsonModelOperation {
    UpsertAux {
        aux: engine::json::Auxiliary,
    },
    UpsertStock {
        stock: engine::json::Stock,
    },
    UpsertFlow {
        flow: engine::json::Flow,
    },
    UpsertModule {
        module: engine::json::Module,
    },
    DeleteVariable {
        ident: String,
    },
    RenameVariable {
        from: String,
        to: String,
    },
    UpsertView {
        index: u32,
        view: engine::json::View,
    },
    DeleteView {
        index: u32,
    },
    UpdateStockFlows {
        ident: String,
        inflows: Vec<String>,
        outflows: Vec<String>,
    },
}

// ── conversion helpers ─────────────────────────────────────────────────

fn convert_json_project_patch(
    patch: JsonProjectPatch,
) -> std::result::Result<engine::ProjectPatch, FfiError> {
    let mut project_ops = Vec::with_capacity(patch.project_ops.len());
    for op in patch.project_ops {
        project_ops.push(convert_json_project_operation(op)?);
    }

    let mut models = Vec::with_capacity(patch.models.len());
    for model in patch.models {
        let mut ops = Vec::with_capacity(model.ops.len());
        for op in model.ops {
            ops.push(convert_json_model_operation(op)?);
        }
        models.push(engine::ModelPatch {
            name: model.name,
            ops,
        });
    }

    Ok(engine::ProjectPatch {
        project_ops,
        models,
    })
}

fn convert_json_project_operation(
    op: JsonProjectOperation,
) -> std::result::Result<engine::ProjectOperation, FfiError> {
    let result = match op {
        JsonProjectOperation::SetSimSpecs { sim_specs } => {
            engine::ProjectOperation::SetSimSpecs(sim_specs.into())
        }
        JsonProjectOperation::AddModel { name } => engine::ProjectOperation::AddModel { name },
    };
    Ok(result)
}

fn convert_json_model_operation(
    op: JsonModelOperation,
) -> std::result::Result<engine::ModelOperation, FfiError> {
    let result = match op {
        JsonModelOperation::UpsertAux { aux } => engine::ModelOperation::UpsertAux(aux.into()),
        JsonModelOperation::UpsertStock { stock } => {
            engine::ModelOperation::UpsertStock(stock.into())
        }
        JsonModelOperation::UpsertFlow { flow } => engine::ModelOperation::UpsertFlow(flow.into()),
        JsonModelOperation::UpsertModule { module } => {
            engine::ModelOperation::UpsertModule(module.into())
        }
        JsonModelOperation::DeleteVariable { ident } => {
            engine::ModelOperation::DeleteVariable { ident }
        }
        JsonModelOperation::RenameVariable { from, to } => {
            engine::ModelOperation::RenameVariable { from, to }
        }
        JsonModelOperation::UpsertView { index, view } => engine::ModelOperation::UpsertView {
            index,
            view: view.into(),
        },
        JsonModelOperation::DeleteView { index } => engine::ModelOperation::DeleteView { index },
        JsonModelOperation::UpdateStockFlows {
            ident,
            inflows,
            outflows,
        } => engine::ModelOperation::UpdateStockFlows {
            ident,
            inflows,
            outflows,
        },
    };
    Ok(result)
}

// ── ErrorDetailBuilder ─────────────────────────────────────────────────

// Builder for error details used to populate SimlinError instances
struct ErrorDetailBuilder {
    code: SimlinErrorCode,
    message: Option<String>,
    model_name: Option<String>,
    variable_name: Option<String>,
    start_offset: u16,
    end_offset: u16,
    kind: SimlinErrorKind,
    unit_error_kind: SimlinUnitErrorKind,
}

impl ErrorDetailBuilder {
    fn new(code: ErrorCode) -> Self {
        Self {
            code: SimlinErrorCode::from(code),
            message: None,
            model_name: None,
            variable_name: None,
            start_offset: 0,
            end_offset: 0,
            kind: SimlinErrorKind::default(),
            unit_error_kind: SimlinUnitErrorKind::default(),
        }
    }

    fn message(mut self, msg: Option<String>) -> Self {
        self.message = msg;
        self
    }

    fn model_name(mut self, name: &str) -> Self {
        self.model_name = Some(name.to_string());
        self
    }

    fn variable_name(mut self, name: &str) -> Self {
        self.variable_name = Some(name.to_string());
        self
    }

    fn offsets(mut self, start: u16, end: u16) -> Self {
        self.start_offset = start;
        self.end_offset = end;
        self
    }

    fn kind(mut self, kind: SimlinErrorKind) -> Self {
        self.kind = kind;
        self
    }

    fn unit_error_kind(mut self, unit_error_kind: SimlinUnitErrorKind) -> Self {
        self.unit_error_kind = unit_error_kind;
        self
    }

    fn build(self) -> ErrorDetailData {
        ErrorDetailData {
            code: self.code,
            message: self.message,
            model_name: self.model_name,
            variable_name: self.variable_name,
            start_offset: self.start_offset,
            end_offset: self.end_offset,
            kind: self.kind,
            unit_error_kind: self.unit_error_kind,
        }
    }

    fn from_formatted(error: errors::FormattedError) -> ErrorDetailData {
        let kind = match error.kind {
            errors::FormattedErrorKind::Project => SimlinErrorKind::Project,
            errors::FormattedErrorKind::Model => SimlinErrorKind::Model,
            errors::FormattedErrorKind::Variable => SimlinErrorKind::Variable,
            errors::FormattedErrorKind::Units => SimlinErrorKind::Units,
            errors::FormattedErrorKind::Simulation => SimlinErrorKind::Simulation,
        };
        let unit_error_kind = match error.unit_error_kind {
            Some(errors::UnitErrorKind::Definition) => SimlinUnitErrorKind::Definition,
            Some(errors::UnitErrorKind::Consistency) => SimlinUnitErrorKind::Consistency,
            Some(errors::UnitErrorKind::Inference) => SimlinUnitErrorKind::Inference,
            None => SimlinUnitErrorKind::NotApplicable,
        };
        let mut builder = ErrorDetailBuilder::new(error.code)
            .kind(kind)
            .unit_error_kind(unit_error_kind);
        if let Some(message) = error.message {
            builder = builder.message(Some(message));
        }
        if let Some(model_name) = error.model_name {
            builder = builder.model_name(&model_name);
        }
        if let Some(variable_name) = error.variable_name {
            builder = builder.variable_name(&variable_name);
        }
        builder
            .offsets(error.start_offset, error.end_offset)
            .build()
    }
}

// ── error collection ───────────────────────────────────────────────────

fn collect_project_errors(project: &engine::Project) -> Vec<ErrorDetailData> {
    errors::collect_formatted_issues(project)
        .errors
        .into_iter()
        .map(ErrorDetailBuilder::from_formatted)
        .collect()
}

pub(crate) fn gather_error_details(
    project: &engine::Project,
) -> (Vec<ErrorDetailData>, Option<engine::Error>) {
    let mut all_errors = collect_project_errors(project);
    let sim_error = compile_simulation(project, "main").and_then(Vm::new).err();

    if let Some(error) = sim_error.clone() {
        let formatted = errors::format_simulation_error("main", &error);
        all_errors.push(ErrorDetailBuilder::from_formatted(formatted));
    }

    (all_errors, sim_error)
}

fn first_error_code(
    project: &engine::Project,
    sim_error: Option<&engine::Error>,
) -> Option<SimlinErrorCode> {
    if let Some(error) = project.errors.first() {
        return Some(SimlinErrorCode::from(error.code));
    }

    for model in project.models.values() {
        if let Some(errors) = &model.errors {
            if let Some(error) = errors.first() {
                return Some(SimlinErrorCode::from(error.code));
            }
        }

        if model
            .get_variable_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::VariablesHaveErrors);
        }

        if model
            .get_unit_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::UnitDefinitionErrors);
        }
    }

    sim_error.map(|error| SimlinErrorCode::from(error.code))
}

/// Collects models that have unit warnings as a set of model names.
///
/// We use model names rather than (model, details) tuples because unit inference
/// can produce different details strings (e.g., different variable ordering) for
/// the same underlying issue when the model is recompiled. Comparing by model name
/// ensures that if a model already had unit warnings before a patch, further patches
/// to that model are allowed (even if they don't fix the existing warnings).
fn collect_models_with_unit_warnings(
    project: &engine::Project,
) -> std::collections::HashSet<String> {
    let mut models_with_warnings = std::collections::HashSet::new();

    for (model_name, model) in &project.models {
        if model.unit_warnings.is_some() {
            models_with_warnings.insert(model_name.to_string());
        }
    }

    models_with_warnings
}

// ── patch application ──────────────────────────────────────────────────

/// Internal helper that applies a ProjectPatch to a project.
///
/// This is the core patch application logic. It handles datamodel cloning,
/// patch application, error gathering, validation, and committing changes
/// (unless dry_run is true).
pub(crate) unsafe fn apply_project_patch_internal(
    project_ref: &SimlinProject,
    patch: engine::ProjectPatch,
    dry_run: bool,
    allow_errors: bool,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    // Collect models that already have unit warnings before applying the patch.
    // We only reject patches that introduce warnings in models that were previously clean.
    let models_with_existing_warnings = {
        let project_locked = project_ref.project.lock().unwrap();
        collect_models_with_unit_warnings(&project_locked)
    };

    let mut staged_datamodel = {
        let project_locked = project_ref.project.lock().unwrap();
        project_locked.datamodel.clone()
    };

    if let Err(err) = engine::apply_patch(&mut staged_datamodel, patch) {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::from(err.code))
                .with_message(format!("failed to apply patch: {err}")),
        );
        return;
    }

    let staged_project = engine::Project::from(staged_datamodel);

    let (all_errors, sim_error) = gather_error_details(&staged_project);

    // Check for blocking errors (not including unit warnings, which are handled separately)
    let maybe_first_code = if !allow_errors {
        first_error_code(&staged_project, sim_error.as_ref())
    } else {
        None
    };

    // Check for NEW unit warnings in models that were previously clean.
    // If a model already had unit warnings, further changes to it are allowed.
    let new_unit_warning = if !allow_errors && maybe_first_code.is_none() {
        let models_with_new_warnings = collect_models_with_unit_warnings(&staged_project);
        // Find models that now have warnings but didn't before
        models_with_new_warnings
            .difference(&models_with_existing_warnings)
            .next()
            .map(|model_name| {
                (
                    SimlinErrorCode::UnitMismatch,
                    format!(
                        "patch introduces unit warning in model '{}' which previously had none",
                        model_name
                    ),
                )
            })
    } else {
        None
    };

    if !out_collected_errors.is_null() && !all_errors.is_empty() {
        let code = maybe_first_code
            .or(new_unit_warning.as_ref().map(|(code, _)| *code))
            .or_else(|| all_errors.first().map(|detail| detail.code))
            .unwrap_or(SimlinErrorCode::NoError);
        let aggregate = build_simlin_error(code, &all_errors);
        *out_collected_errors = aggregate.into_raw();
    }

    if let Some(code) = maybe_first_code {
        let error = build_simlin_error(code, &all_errors);
        store_error(out_error, error);
        return;
    }

    if let Some((code, message)) = new_unit_warning {
        store_error(out_error, SimlinError::new(code).with_message(message));
        return;
    }

    if !dry_run {
        let mut project_locked = project_ref.project.lock().unwrap();
        *project_locked = staged_project;
    }
}

// ── FFI entry point ────────────────────────────────────────────────────

/// Applies a JSON patch to the project datamodel.
///
/// # Safety
/// - `project` must point to a valid `SimlinProject`.
/// - `patch_data` must either be null with `patch_len == 0` or reference at
///   least `patch_len` bytes containing UTF-8 JSON.
/// - `out_collected_errors` and `out_error` must be valid pointers for writing
///   error details and may be set to null on success.
///
/// # Thread Safety
/// - This function is thread-safe for concurrent calls with the same `project` pointer.
/// - The underlying `engine::Project` uses `Arc<ModelStage1>` and is protected by a `Mutex`.
/// - Multiple threads may safely modify the same project concurrently.
/// - Different projects may also be patched concurrently from different threads safely.
///
/// # Ownership and Mutation
/// - When `dry_run` is false, this function modifies the project in-place.
/// - When `dry_run` is true, the project remains unchanged and no modifications are committed.
/// - The `project` pointer remains valid and usable after this function returns.
/// - The project is not consumed or moved by this operation.
#[no_mangle]
pub unsafe extern "C" fn simlin_project_apply_patch(
    project: *mut SimlinProject,
    patch_data: *const u8,
    patch_len: usize,
    dry_run: bool,
    allow_errors: bool,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !out_collected_errors.is_null() {
        *out_collected_errors = ptr::null_mut();
    }

    if patch_len > 0 && patch_data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("patch_data pointer must not be NULL when patch_len > 0"),
        );
        return;
    }

    let patch_slice = if patch_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(patch_data, patch_len)
    };

    let json_str = match std::str::from_utf8(patch_slice) {
        Ok(s) => s,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("input JSON is not valid UTF-8: {err}")),
            );
            return;
        }
    };

    // Treat empty input as a valid no-op patch (maintains backwards compatibility
    // with callers that pass NULL+0 for "no changes")
    let json_str = if json_str.trim().is_empty() {
        r#"{"projectOps":[],"models":[]}"#
    } else {
        json_str
    };

    let json_patch: JsonProjectPatch = match serde_json::from_str(json_str) {
        Ok(patch) => patch,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to parse JSON patch: {err}")),
            );
            return;
        }
    };

    let patch = match convert_json_project_patch(json_patch) {
        Ok(patch) => patch,
        Err(err) => {
            store_ffi_error(out_error, err);
            return;
        }
    };

    let project_ref = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    apply_project_patch_internal(
        project_ref,
        patch,
        dry_run,
        allow_errors,
        out_collected_errors,
        out_error,
    );
}
