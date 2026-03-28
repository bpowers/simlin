// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Layout FFI function.
//!
//! Exposes the automatic diagram layout engine through the C FFI.

use simlin_engine::{self as engine};
use std::ffi::CStr;
use std::os::raw::c_char;

use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::patch::{convert_json_project_patch, JsonProjectPatch};
use crate::{clear_out_error, require_project, store_error, SimlinErrorCode, SimlinProject};

/// Generate the best automatic layout for the named model and replace its
/// views in-place.
///
/// When `patch_json` is non-NULL, deserializes it as a JSON project patch
/// and uses incremental layout (preserving existing element positions) if
/// the model already has a non-empty view.  When NULL, always generates a
/// full layout from scratch.
///
/// Preserves the existing zoom level if the model already has a view with
/// zoom > 0. Works on all targets including WASM (uses a serial fallback
/// when rayon is unavailable).
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` must be a valid null-terminated UTF-8 string
/// - `patch_json` may be null; when non-null must be a valid null-terminated UTF-8 JSON string
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_diagram_sync(
    project: *mut SimlinProject,
    model_name: *const c_char,
    patch_json: *const c_char,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);

    let proj = ffi_try!(out_error, require_project(project));

    if model_name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("model name pointer must not be NULL"),
        );
        return;
    }

    let model_name_str = match CStr::from_ptr(model_name).to_str() {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("model name must not be empty"),
            );
            return;
        }
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("model name is not valid UTF-8"),
            );
            return;
        }
    };

    // Deserialize the optional patch JSON up front, before acquiring locks.
    let model_patch = if !patch_json.is_null() {
        let json_str = match CStr::from_ptr(patch_json).to_str() {
            Ok(s) => s,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("patch_json is not valid UTF-8"),
                );
                return;
            }
        };
        let json_patch: JsonProjectPatch = match serde_json::from_str(json_str) {
            Ok(p) => p,
            Err(e) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message(format!("failed to parse patch_json: {e}")),
                );
                return;
            }
        };
        let engine_patch = match convert_json_project_patch(json_patch) {
            Ok(p) => p,
            Err(e) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message(format!("failed to convert patch: {e}")),
                );
                return;
            }
        };
        // Collect all patches for this model and merge their ops, because a
        // ProjectPatch may legally contain multiple ModelPatch entries for the
        // same model (e.g. two separate UpsertFlow ops on the same model).
        // Using find() would silently drop all but the first matching entry.
        // Mirror the alias logic in datamodel::Project::get_model: treat
        // "main" and "" as equivalent so that patches using the stored model
        // name ("") are matched when the caller passes "main".
        let matching_ops: Vec<_> = engine_patch
            .models
            .iter()
            .filter(|m| m.name == model_name_str || (model_name_str == "main" && m.name.is_empty()))
            .flat_map(|m| m.ops.iter().cloned())
            .collect();
        if matching_ops.is_empty() {
            None
        } else {
            Some(simlin_engine::ModelPatch {
                name: model_name_str.to_string(),
                ops: matching_ops,
            })
        }
    } else {
        None
    };

    let mut datamodel_locked = proj.datamodel.lock().unwrap();

    // Check model existence up front so we can distinguish "not found"
    // (DoesNotExist) from internal layout failures (Generic).
    if datamodel_locked.get_model(model_name_str).is_none() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist)
                .with_message(format!("model '{}' not found in project", model_name_str)),
        );
        return;
    }

    // Extract old view info before generating layout
    let old_view = datamodel_locked
        .get_model(model_name_str)
        .and_then(|m| m.views.first())
        .map(|v| match v {
            engine::datamodel::View::StockFlow(sf) => sf,
        });

    let existing_zoom = old_view.map(|sf| sf.zoom).filter(|&z| z > 0.0);

    // Layout generation requires the salsa db for dependency extraction
    // and LTM analysis. The project must have been synced first.
    let mut db_locked = proj.db.lock().unwrap();
    let sync_locked = proj.sync_state.lock().unwrap();
    let source_project = match sync_locked.as_ref() {
        Some(state) => state.project,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("project must be synced before layout generation"),
            );
            return;
        }
    };
    drop(sync_locked);

    let db_state = Some((&mut *db_locked, source_project));

    // Use incremental layout when a patch and existing non-empty view are available
    let new_view = if let (Some(old_sf), Some(ref mp)) = (old_view, &model_patch) {
        if !old_sf.elements.is_empty() {
            // Clone required: old_sf borrows from datamodel_locked, but
            // incremental_layout also needs &datamodel_locked for the project.
            let old_sf = old_sf.clone();
            engine::layout::incremental_layout(
                &old_sf,
                &datamodel_locked,
                model_name_str,
                mp,
                db_state,
            )
        } else {
            engine::layout::generate_best_layout(&datamodel_locked, model_name_str, db_state)
        }
    } else {
        engine::layout::generate_best_layout(&datamodel_locked, model_name_str, db_state)
    };

    let mut layout = match new_view {
        Ok(l) => l,
        Err(msg) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(msg),
            );
            return;
        }
    };

    if let Some(zoom) = existing_zoom {
        layout.zoom = zoom;
    }

    // Model existence was verified above, so this should always succeed.
    let model = datamodel_locked.get_model_mut(model_name_str).unwrap();
    model.views = vec![engine::datamodel::View::StockFlow(layout)];
}
