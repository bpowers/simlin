// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// `ProjectDoc` wraps a `LoroDoc` and exposes a small, project-shaped surface:
// a single `apply_canonical_json` write primitive plus an inverse exporter.
// All cross-source writes (HTTP save, MCP edits, file watcher reload) flow
// through the same merge call so the CRDT machinery can see every op.
//
// The `LoroDoc` itself has interior mutability (its operations take `&self`),
// so this module is technically "Functional Core" only modulo that single
// piece of opaque state. The merge primitive is a pure function over the
// incoming JSON shape and the doc's current observed state — there is no
// other I/O.

//! Per-project Loro CRDT document and the merge primitive that backs all
//! writes against it.
//!
//! Shape: the doc's root is a `LoroMap` named `"project"` mirroring
//! `simlin_engine::json::Project`. Scalar fields land directly; the
//! `models` array is reshaped into a name-keyed map at merge time, and
//! within each model the variable arrays (`stocks`/`flows`/`auxiliaries`/
//! `modules`) are likewise reshaped to canonical-name keys. This is what
//! gives us per-variable last-writer-wins on concurrent edits. The
//! `views` list is preserved in array form (positions are meaningful for
//! views).
//!
//! `apply_canonical_json` is the only mutator. It walks the incoming
//! `serde_json::Value` against the live `LoroMap`/`LoroList` state and
//! emits the minimal op set: scalar updates, deletions for missing keys,
//! container insertion + recursion for nested objects, and replace-and-
//! repush for lists. A single `commit()` is fired at the end of the call
//! so a hypothetical `subscribe_root` callback (added by Phase 4) sees
//! one event per merge.

use std::collections::HashSet;

use loro::{Container, LoroDoc, LoroList, LoroMap, LoroValue, ValueOrContainer};
use serde_json::{Map as JsonMap, Number, Value};
use simlin_engine::common::{Canonical, Ident};

/// Errors raised while diffing or merging JSON against the Loro tree.
///
/// `LoroError` wraps any failure surfaced by the CRDT runtime (most often
/// "key not found" on a delete or "type mismatch" when re-keying). `JsonError`
/// covers serde_json failures during the `String` <-> `Value` conversions used
/// at the boundaries. `ShapeError` is the structural-mismatch case — the
/// caller passed JSON whose shape doesn't fit the project schema (e.g. a
/// string where an object was expected).
#[derive(Debug)]
pub enum MergeError {
    LoroError(loro::LoroError),
    JsonError(serde_json::Error),
    ShapeError {
        path: String,
        expected: &'static str,
        actual: &'static str,
    },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::LoroError(e) => write!(f, "loro error: {e}"),
            MergeError::JsonError(e) => write!(f, "json error: {e}"),
            MergeError::ShapeError {
                path,
                expected,
                actual,
            } => write!(
                f,
                "shape mismatch at {path}: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for MergeError {}

impl From<loro::LoroError> for MergeError {
    fn from(e: loro::LoroError) -> Self {
        MergeError::LoroError(e)
    }
}

impl From<serde_json::Error> for MergeError {
    fn from(e: serde_json::Error) -> Self {
        MergeError::JsonError(e)
    }
}

/// Per-project in-memory Loro document.
///
/// Constructed empty; the first `apply_canonical_json` populates it with
/// the project's parsed shape. Subsequent applies emit minimal op-sets
/// against the live state. `export_canonical_json` is the inverse:
/// snapshot the doc's deep value, inflate map-keyed-by-name shapes back
/// into arrays sorted by canonical name.
#[derive(Debug)]
pub struct ProjectDoc {
    doc: LoroDoc,
}

/// Root container key for the project tree. Kept as a single named map so
/// the doc's persistence layer (whenever we add it) has a stable container
/// id to anchor against.
const ROOT_MAP_KEY: &str = "project";

impl ProjectDoc {
    /// Construct an empty `ProjectDoc`. The underlying `LoroDoc` is created
    /// with no containers; the first `apply_canonical_json` materializes the
    /// `project` root map.
    pub fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
        }
    }

    /// Diff `new_json` against the current state and emit the minimal op-set.
    /// `new_json` must be a JSON object (the project root). Returns
    /// `ShapeError` otherwise.
    ///
    /// One `commit()` per call: Loro batches the inserts/deletes between
    /// commits, so subscribe_root callbacks (Phase 4) fire once per logical
    /// merge regardless of how deep the tree is.
    ///
    /// The incoming JSON is reshaped via `project_json_to_loro_shape`
    /// before merging so variable arrays land as canonical-name-keyed
    /// `LoroMap`s (giving us per-variable LWW on concurrent edits). The
    /// `export_canonical_json` inverse rebuilds the on-disk array shape.
    pub fn apply_canonical_json(&self, new_json: &Value) -> Result<(), MergeError> {
        let projected = project_json_to_loro_shape(new_json)?;
        let json_obj = match &projected {
            Value::Object(o) => o,
            other => {
                return Err(MergeError::ShapeError {
                    path: "$".into(),
                    expected: "object",
                    actual: json_value_kind(other),
                });
            }
        };
        let root = self.doc.get_map(ROOT_MAP_KEY);
        merge_map(&root, json_obj, "$")?;
        self.doc.commit();
        Ok(())
    }

    /// Inverse of `apply_canonical_json`: snapshot the doc's current
    /// project state as a `serde_json::Value`. The returned value matches
    /// the shape that was last applied — i.e. the project object directly,
    /// not wrapped under a `"project"` key — and the variable-array
    /// fields are sorted by name for deterministic output.
    pub fn export_canonical_json(&self) -> Result<Value, MergeError> {
        let deep = self.doc.get_deep_value();
        // The doc's deep value is `{ "project": {...} }`; we strip the
        // `project` wrapper so callers see the same shape they applied.
        // An empty doc (no apply_canonical_json yet) has no `project` key,
        // so we return an empty object as the "no project loaded yet"
        // signal — matches the empty-input case for round-tripping.
        let project_value = match loro_value_to_json(&deep) {
            Value::Object(mut map) => match map.remove(ROOT_MAP_KEY) {
                Some(project) => project,
                None => return Ok(Value::Object(JsonMap::new())),
            },
            other => {
                return Err(MergeError::ShapeError {
                    path: "$".into(),
                    expected: "object",
                    actual: json_value_kind(&other),
                });
            }
        };
        loro_shape_to_canonical_json(project_value)
    }

    /// Convenience: serialize `export_canonical_json` to a JSON string.
    /// The returned shape matches what the SPA expects on `GET /api/projects/...`.
    pub fn current_state_as_json_string(&self) -> Result<String, MergeError> {
        let value = self.export_canonical_json()?;
        Ok(serde_json::to_string(&value)?)
    }

    /// Test-only access to the inner LoroDoc, allowing inline tests to
    /// poke at the doc state directly to verify wrapper plumbing without
    /// going through the public merge primitive (which is being built up
    /// task-by-task in Phase 3).
    #[cfg(test)]
    fn inner_doc(&self) -> &LoroDoc {
        &self.doc
    }
}

impl Default for ProjectDoc {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a `LoroValue` (returned by `LoroDoc::get_deep_value()`) into
/// a `serde_json::Value`. The mapping is total: every variant has a
/// `serde_json` analog, and `Container` is unreachable for `get_deep_value`
/// output (which inlines container state) so we surface it as `Null`
/// defensively rather than panicking.
fn loro_value_to_json(value: &LoroValue) -> Value {
    match value {
        LoroValue::Null => Value::Null,
        LoroValue::Bool(b) => Value::Bool(*b),
        LoroValue::I64(i) => Value::Number((*i).into()),
        LoroValue::Double(d) => Number::from_f64(*d)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        LoroValue::String(s) => Value::String(s.as_str().to_owned()),
        LoroValue::Binary(b) => Value::Array(
            b.iter()
                .map(|byte| Value::Number((*byte as u64).into()))
                .collect(),
        ),
        LoroValue::List(items) => Value::Array(items.iter().map(loro_value_to_json).collect()),
        LoroValue::Map(entries) => {
            let mut map = JsonMap::with_capacity(entries.len());
            for (k, v) in entries.iter() {
                map.insert(k.clone(), loro_value_to_json(v));
            }
            Value::Object(map)
        }
        // `Container` only appears in `get_value()` (shallow). For
        // `get_deep_value()` (which we use exclusively) all containers are
        // inlined as their materialized shape, so reaching this arm would
        // signal an upstream API change.
        LoroValue::Container(_) => Value::Null,
    }
}

/// Convert a `serde_json::Value` to a `LoroValue` for use in scalar
/// `LoroMap::insert` calls. Objects and arrays are projected onto the
/// matching `LoroValue` variants — the merge primitive doesn't use this
/// directly for nested structure (it builds containers via
/// `insert_container` instead), but we use it for scalar inserts and for
/// no-op equality checks.
fn json_to_loro_value(value: &Value) -> LoroValue {
    match value {
        Value::Null => LoroValue::Null,
        Value::Bool(b) => LoroValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                LoroValue::I64(i)
            } else if let Some(f) = n.as_f64() {
                LoroValue::Double(f)
            } else {
                LoroValue::Null
            }
        }
        Value::String(s) => LoroValue::String(s.as_str().into()),
        Value::Array(items) => {
            let v: Vec<LoroValue> = items.iter().map(json_to_loro_value).collect();
            LoroValue::List(v.into())
        }
        Value::Object(map) => {
            let mut out: std::collections::HashMap<String, LoroValue> =
                std::collections::HashMap::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), json_to_loro_value(v));
            }
            LoroValue::Map(out.into())
        }
    }
}

/// Reconcile a `LoroMap` against an incoming JSON object.
///
/// Algorithm:
/// 1. Collect the live key set so iteration order doesn't matter (and so
///    we can mutate during the second pass without invalidating an
///    iterator).
/// 2. Delete any live key the incoming JSON doesn't have. JSON `null`
///    values are also treated as deletions, matching the design's
///    "explicit absence" semantics.
/// 3. For each remaining incoming key:
///    - **Scalar** (bool/number/string): if the live entry equals the new
///      value, skip (no-op suppression keeps the doc's op-log lean).
///      Otherwise insert.
///    - **Object**: if the live entry is already a sub-`LoroMap`, recurse
///      into it. Otherwise (missing, or wrong type) `insert_container`
///      a fresh `LoroMap` and recurse into the new one.
///    - **Array**: delegate to `merge_list`, which uses replace-and-repush
///      semantics on a `LoroList` container.
///
/// The `path` parameter is appended-only so `ShapeError` messages can
/// point at the offending location (`.models[2].stocks` etc.).
fn merge_map(map: &LoroMap, json: &JsonMap<String, Value>, path: &str) -> Result<(), MergeError> {
    let incoming_keys: HashSet<&str> = json
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, _)| k.as_str())
        .collect();

    // Collect live keys first; for_each must not mutate the map under the
    // same iterator.
    let live_keys: HashSet<String> = {
        let mut keys = HashSet::new();
        map.for_each(|k, _| {
            keys.insert(k.to_string());
        });
        keys
    };

    // Null-valued incoming keys are treated as explicit deletions. Keys that
    // are live but absent from the non-null incoming set are also deleted.
    // Union both cases and delete once per key.
    let null_keys: HashSet<&str> = json
        .iter()
        .filter(|(_, v)| v.is_null())
        .map(|(k, _)| k.as_str())
        .collect();
    for key in live_keys
        .iter()
        .filter(|k| !incoming_keys.contains(k.as_str()))
        .map(|k| k.as_str())
        .chain(null_keys.iter().copied())
        .collect::<HashSet<&str>>()
    {
        // delete returns Err for missing keys in some Loro versions;
        // ignore that failure mode since the post-condition (key absent)
        // holds either way.
        let _ = map.delete(key);
    }

    for (key, value) in json {
        if value.is_null() {
            // Deletion already handled in the unified delete pass above.
            continue;
        }

        match value {
            Value::Object(child_obj) => {
                let child_map = match map.get(key) {
                    Some(ValueOrContainer::Container(Container::Map(existing))) => existing,
                    Some(_) | None => {
                        // Replace any non-map entry (or freshly insert) so
                        // we always have a sub-map to recurse into. Loro
                        // overwrites the prior value transparently.
                        map.insert_container(key, LoroMap::new())?
                    }
                };
                let child_path = append_path(path, key);
                merge_map(&child_map, child_obj, &child_path)?;
            }
            Value::Array(child_arr) => {
                let child_list = match map.get(key) {
                    Some(ValueOrContainer::Container(Container::List(existing))) => existing,
                    Some(_) | None => map.insert_container(key, LoroList::new())?,
                };
                let child_path = append_path(path, key);
                merge_list(&child_list, child_arr, &child_path)?;
            }
            // Scalar branch.
            Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                let new_val = json_to_loro_value(value);
                let same = match map.get(key) {
                    Some(ValueOrContainer::Value(existing)) => existing == new_val,
                    _ => false,
                };
                if !same {
                    map.insert(key, new_val)?;
                }
            }
            // Null was handled above; `Value` has no other variants.
            Value::Null => unreachable!("null filtered above"),
        }
    }

    Ok(())
}

/// Phase 3 trade-off: lists use replace-semantics; reorderings show as
/// full-list rewrites against the CRDT op-log. Acceptable because (a)
/// the only top-level lists in the project shape are `views`/`dimensions`/
/// `units`/`groups`, all small and edited infrequently relative to
/// variables, and (b) preserving uid-based identity through a
/// `LoroMovableList` is significant complexity for a benefit (per-element
/// LWW on view layout) we don't need yet. Task 4 puts variables in
/// name-keyed maps to recover the per-variable LWW property where it
/// actually matters.
///
/// `merge_list` truncates the live list to zero, then re-pushes the
/// incoming elements. Container elements (objects, nested arrays) are
/// pushed via `push_container` and recursed into.
fn merge_list(list: &LoroList, json: &[Value], path: &str) -> Result<(), MergeError> {
    let len = list.len();
    if len > 0 {
        list.delete(0, len)?;
    }
    for (idx, value) in json.iter().enumerate() {
        match value {
            Value::Object(child_obj) => {
                let child_map = list.push_container(LoroMap::new())?;
                let child_path = format!("{path}[{idx}]");
                merge_map(&child_map, child_obj, &child_path)?;
            }
            Value::Array(child_arr) => {
                let child_list = list.push_container(LoroList::new())?;
                let child_path = format!("{path}[{idx}]");
                merge_list(&child_list, child_arr, &child_path)?;
            }
            _ => {
                list.push(json_to_loro_value(value))?;
            }
        }
    }
    Ok(())
}

fn append_path(path: &str, key: &str) -> String {
    if path == "$" {
        format!(".{key}")
    } else {
        format!("{path}.{key}")
    }
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Variable-array fields within a `json::Model` that we project into
/// canonical-name-keyed maps. Listed in source-of-truth order so any
/// future variant added to `json::Model` can be cross-checked at a
/// glance. Lists outside this set (`views`, `loop_metadata`, `groups`)
/// stay as positional arrays since they don't carry a stable name key.
const VARIABLE_ARRAY_FIELDS: &[&str] = &["stocks", "flows", "auxiliaries", "modules"];

/// Re-shape a `json::Project`-shaped value for storage in the LoroDoc.
///
/// The on-disk JSON uses `Vec<Variable>` for stocks/flows/auxiliaries/
/// modules and `Vec<Model>` for the models. In Loro we want the per-name
/// last-writer-wins property, which `LoroMap` gives us natively when the
/// keys are stable identifiers. So the projection rewrites:
///
///   - `models: [<Model>, ...]`  -> `models: { "<model_name>": <Model> }`
///   - inside each model:
///     `stocks: [<Stock>, ...]` -> `stocks: { "<canonical_name>": <Stock> }`
///     (and same for flows / auxiliaries / modules)
///
/// `views`/`dimensions`/`units`/`groups`/`loop_metadata` are left alone
/// as positional arrays — they have no stable per-element name key (or
/// position is the meaningful field, as for `views`).
///
/// Variable names are canonicalized via `Ident::<Canonical>::new(...)`
/// so callers using mixed-case or whitespace variants land on the same
/// key. Model names are stored as-is — the design doesn't canonicalize
/// model names elsewhere either.
///
/// Returns `ShapeError` only when the input fundamentally doesn't match
/// the project schema (e.g. `models` isn't an array). Missing optional
/// keys are passed through unchanged so partial projects (a JSON missing
/// `dimensions`, say) still merge cleanly.
fn project_json_to_loro_shape(json: &Value) -> Result<Value, MergeError> {
    let obj = match json {
        Value::Object(map) => map,
        _ => {
            return Err(MergeError::ShapeError {
                path: "$".into(),
                expected: "object",
                actual: json_value_kind(json),
            });
        }
    };

    let mut out = JsonMap::with_capacity(obj.len());
    for (key, value) in obj {
        if key == "models" {
            out.insert(key.clone(), models_array_to_map(value, "$.models")?);
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn models_array_to_map(value: &Value, path: &str) -> Result<Value, MergeError> {
    let arr = match value {
        Value::Array(a) => a,
        _ => {
            return Err(MergeError::ShapeError {
                path: path.into(),
                expected: "array",
                actual: json_value_kind(value),
            });
        }
    };

    let mut out = JsonMap::with_capacity(arr.len());
    for (idx, model) in arr.iter().enumerate() {
        let elem_path = format!("{path}[{idx}]");
        let model_obj = match model {
            Value::Object(m) => m,
            _ => {
                return Err(MergeError::ShapeError {
                    path: elem_path,
                    expected: "object",
                    actual: json_value_kind(model),
                });
            }
        };
        let model_name = model_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MergeError::ShapeError {
                path: format!("{elem_path}.name"),
                expected: "string",
                actual: model_obj
                    .get("name")
                    .map(json_value_kind)
                    .unwrap_or("missing"),
            })?
            .to_owned();
        let projected_model = project_model(model_obj, &elem_path)?;
        out.insert(model_name, projected_model);
    }
    Ok(Value::Object(out))
}

fn project_model(model: &JsonMap<String, Value>, path: &str) -> Result<Value, MergeError> {
    let mut out = JsonMap::with_capacity(model.len());
    for (key, value) in model {
        if VARIABLE_ARRAY_FIELDS.contains(&key.as_str()) {
            let field_path = format!("{path}.{key}");
            out.insert(key.clone(), variables_array_to_map(value, &field_path)?);
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn variables_array_to_map(value: &Value, path: &str) -> Result<Value, MergeError> {
    let arr = match value {
        Value::Array(a) => a,
        _ => {
            return Err(MergeError::ShapeError {
                path: path.into(),
                expected: "array",
                actual: json_value_kind(value),
            });
        }
    };

    let mut out = JsonMap::with_capacity(arr.len());
    for (idx, var) in arr.iter().enumerate() {
        let elem_path = format!("{path}[{idx}]");
        let var_obj = match var {
            Value::Object(v) => v,
            _ => {
                return Err(MergeError::ShapeError {
                    path: elem_path,
                    expected: "object",
                    actual: json_value_kind(var),
                });
            }
        };
        let raw_name = var_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MergeError::ShapeError {
                path: format!("{elem_path}.name"),
                expected: "string",
                actual: var_obj
                    .get("name")
                    .map(json_value_kind)
                    .unwrap_or("missing"),
            })?;
        let key = Ident::<Canonical>::new(raw_name).as_str().to_owned();
        out.insert(key, Value::Object(var_obj.clone()));
    }
    Ok(Value::Object(out))
}

/// Inverse of `project_json_to_loro_shape`: rebuild the on-disk
/// `Vec<Variable>` / `Vec<Model>` arrays from the name-keyed maps.
///
/// Sort order: variables and models are sorted by their `name` field
/// (case-sensitive lexicographic). This mirrors the order XMILE writers
/// produce so a roundtrip via the LoroDoc doesn't churn the on-disk
/// file. If a variable somehow lacks a `name` (shouldn't happen given
/// `apply_canonical_json` enforces the schema on write), we sort it
/// to the end with the canonical map key as a fallback.
fn loro_shape_to_canonical_json(value: Value) -> Result<Value, MergeError> {
    let obj = match value {
        Value::Object(map) => map,
        _ => {
            return Err(MergeError::ShapeError {
                path: "$".into(),
                expected: "object",
                actual: json_value_kind(&value),
            });
        }
    };

    let mut out = JsonMap::with_capacity(obj.len());
    for (key, value) in obj {
        if key == "models" {
            out.insert(key, models_map_to_array(value, "$.models")?);
        } else {
            out.insert(key, value);
        }
    }
    Ok(Value::Object(out))
}

fn models_map_to_array(value: Value, path: &str) -> Result<Value, MergeError> {
    let map = match value {
        Value::Object(m) => m,
        _ => {
            return Err(MergeError::ShapeError {
                path: path.into(),
                expected: "object",
                actual: json_value_kind(&value),
            });
        }
    };
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::with_capacity(entries.len());
    for (key, model_value) in entries {
        let model_obj = match model_value {
            Value::Object(m) => m,
            other => {
                return Err(MergeError::ShapeError {
                    path: format!("{path}.{key}"),
                    expected: "object",
                    actual: json_value_kind(&other),
                });
            }
        };
        let model_path = format!("{path}.{key}");
        out.push(unproject_model(model_obj, &model_path)?);
    }
    Ok(Value::Array(out))
}

fn unproject_model(model: JsonMap<String, Value>, path: &str) -> Result<Value, MergeError> {
    let mut out = JsonMap::with_capacity(model.len());
    for (key, value) in model {
        if VARIABLE_ARRAY_FIELDS.contains(&key.as_str()) {
            let field_path = format!("{path}.{key}");
            out.insert(key, variables_map_to_array(value, &field_path)?);
        } else {
            out.insert(key, value);
        }
    }
    Ok(Value::Object(out))
}

fn variables_map_to_array(value: Value, path: &str) -> Result<Value, MergeError> {
    let map = match value {
        Value::Object(m) => m,
        _ => {
            return Err(MergeError::ShapeError {
                path: path.into(),
                expected: "object",
                actual: json_value_kind(&value),
            });
        }
    };
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    // Sort by the variable's `name` field so the on-disk array matches
    // the natural alphabetic order used by the XMILE writer. Keys of
    // the canonical-name map are themselves canonical, so we fall back
    // to them when `name` is missing (defensive — won't happen for
    // doc state produced through the schema-enforcing apply path).
    entries.sort_by(|a, b| {
        let name_a =
            a.1.as_object()
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(a.0.as_str());
        let name_b =
            b.1.as_object()
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(b.0.as_str());
        name_a.cmp(name_b)
    });
    let out: Vec<Value> = entries.into_iter().map(|(_, v)| v).collect();
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_project_doc_exports_empty_object() {
        // A fresh ProjectDoc has no containers; get_deep_value returns an
        // empty map shape that round-trips to a JSON object with no keys.
        let doc = ProjectDoc::new();
        let exported = doc.export_canonical_json().expect("export");
        // Loro's empty-doc deep value is an empty object `{}`.
        let expected = serde_json::json!({});
        assert_eq!(exported, expected);
    }

    #[test]
    fn current_state_as_json_string_serializes_empty_object() {
        let doc = ProjectDoc::new();
        let s = doc.current_state_as_json_string().expect("string");
        assert_eq!(s, "{}");
    }

    #[test]
    fn manually_inserted_root_key_round_trips_through_export() {
        // Bypass apply_canonical_json and write directly through the inner
        // LoroDoc to prove the export plumbing works end-to-end. The
        // exported value should be the inner project content (not wrapped
        // under a `"project"` key) since `export_canonical_json` strips
        // that wrapper to mirror the apply input shape.
        let doc = ProjectDoc::new();
        let root = doc.inner_doc().get_map(ROOT_MAP_KEY);
        root.insert("name", "demo")
            .expect("insert string into root map");
        doc.inner_doc().commit();

        let exported = doc.export_canonical_json().expect("export");
        let obj = exported.as_object().expect("object root");
        assert_eq!(obj.get("name").and_then(|v| v.as_str()), Some("demo"));
    }

    #[test]
    fn merge_error_display_messages() {
        let shape = MergeError::ShapeError {
            path: ".models[0].stocks".into(),
            expected: "object",
            actual: "string",
        };
        let msg = format!("{shape}");
        assert!(msg.contains(".models[0].stocks"));
        assert!(msg.contains("expected object"));
        assert!(msg.contains("got string"));
    }

    #[test]
    fn apply_canonical_json_rejects_non_object_root() {
        let doc = ProjectDoc::new();
        let err = doc
            .apply_canonical_json(&serde_json::json!("a string, not an object"))
            .unwrap_err();
        match err {
            MergeError::ShapeError {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, "$");
                assert_eq!(expected, "object");
                assert_eq!(actual, "string");
            }
            other => panic!("expected ShapeError, got {other:?}"),
        }
    }

    #[test]
    fn apply_canonical_json_writes_top_level_scalars() {
        let doc = ProjectDoc::new();
        let input = serde_json::json!({ "name": "foo", "version": 1 });
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");
        // The export shape should match the input: top-level scalars only.
        assert_eq!(exported, input);
    }

    #[test]
    fn apply_canonical_json_removes_keys_no_longer_present() {
        let doc = ProjectDoc::new();
        let initial = serde_json::json!({ "name": "foo", "version": 1 });
        doc.apply_canonical_json(&initial).expect("first apply");
        let updated = serde_json::json!({ "name": "bar" });
        doc.apply_canonical_json(&updated).expect("second apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_treats_null_as_deletion() {
        // Explicit `null` should remove a previously-present key, matching
        // the design's "null = absent" convention. We test the round-trip:
        // first apply leaves the key set; second apply with `null` removes it.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "name": "foo", "tag": "old" }))
            .expect("first apply");
        doc.apply_canonical_json(&serde_json::json!({ "name": "foo", "tag": null }))
            .expect("second apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, serde_json::json!({ "name": "foo" }));
    }

    #[test]
    fn apply_canonical_json_handles_nested_objects() {
        let doc = ProjectDoc::new();
        let input = serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice", "year": 2026 },
        });
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, input);
    }

    #[test]
    fn apply_canonical_json_updates_nested_scalar() {
        // Two applies that differ only in a deeply-nested scalar should
        // round-trip. This exercises the recursion path.
        let doc = ProjectDoc::new();
        let first = serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice", "year": 2026 },
        });
        doc.apply_canonical_json(&first).expect("first apply");

        let second = serde_json::json!({
            "name": "foo",
            "meta": { "author": "bob", "year": 2026 },
        });
        doc.apply_canonical_json(&second).expect("second apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, second);
    }

    #[test]
    fn apply_canonical_json_idempotent_for_same_input() {
        // Two applies of the same JSON should leave the export byte-equal.
        // No-op suppression for unchanged scalars helps keep the op log
        // clean; we verify behavior by content-comparing the export. (Loro
        // 1.11 records empty commits even when no ops were emitted, so a
        // version-frontiers comparison is not a reliable purity check —
        // the design's content-equality fallback applies.)
        let doc = ProjectDoc::new();
        let input = serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice" },
        });
        doc.apply_canonical_json(&input).expect("first");
        let after_first = doc.export_canonical_json().expect("export 1");
        doc.apply_canonical_json(&input).expect("second");
        let after_second = doc.export_canonical_json().expect("export 2");
        assert_eq!(after_first, after_second);
        assert_eq!(after_second, input);
    }

    #[test]
    fn apply_canonical_json_replaces_object_with_scalar() {
        // A field that was an object becomes a scalar in the next apply.
        // We must successfully overwrite the prior LoroMap with a scalar
        // rather than failing on the type mismatch.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice" },
        }))
        .expect("first");
        doc.apply_canonical_json(&serde_json::json!({
            "name": "foo",
            "meta": "string-now",
        }))
        .expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(
            exported,
            serde_json::json!({ "name": "foo", "meta": "string-now" })
        );
    }

    #[test]
    fn apply_canonical_json_removes_nested_object_key() {
        // A whole sub-object disappears between applies. Loro must
        // delete the sub-container cleanly, not leave it dangling.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice" },
        }))
        .expect("first");
        doc.apply_canonical_json(&serde_json::json!({ "name": "foo" }))
            .expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, serde_json::json!({ "name": "foo" }));
    }

    #[test]
    fn apply_canonical_json_replaces_scalar_with_object() {
        // Inverse of the above: a scalar becomes an object.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({
            "name": "foo",
            "meta": "scalar",
        }))
        .expect("first");
        doc.apply_canonical_json(&serde_json::json!({
            "name": "foo",
            "meta": { "author": "alice" },
        }))
        .expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(
            exported,
            serde_json::json!({ "name": "foo", "meta": { "author": "alice" } })
        );
    }

    #[test]
    fn apply_canonical_json_writes_scalar_list() {
        let doc = ProjectDoc::new();
        let input = serde_json::json!({ "tags": ["a", "b"] });
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, input);
    }

    #[test]
    fn apply_canonical_json_replaces_list_with_reordered_one() {
        // Replace-semantics: the second apply truncates and repushes, so
        // the export reflects the new order even though the elements are
        // identical to the first apply (set-equality).
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "tags": ["a", "b"] }))
            .expect("first");
        let updated = serde_json::json!({ "tags": ["a", "c", "b"] });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_clears_list_to_empty() {
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "tags": ["a", "b", "c"] }))
            .expect("first");
        let cleared = serde_json::json!({ "tags": [] });
        doc.apply_canonical_json(&cleared).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, cleared);
    }

    #[test]
    fn apply_canonical_json_handles_list_of_objects() {
        let doc = ProjectDoc::new();
        let input = serde_json::json!({
            "items": [{ "id": 1, "label": "first" }, { "id": 2, "label": "second" }],
        });
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, input);
    }

    #[test]
    fn apply_canonical_json_handles_nested_lists() {
        // A list of lists exercises the recursive merge_list -> merge_list
        // path. Each push_container yields a fresh LoroList we recurse into.
        let doc = ProjectDoc::new();
        let input = serde_json::json!({
            "matrix": [[1, 2, 3], [4, 5, 6]],
        });
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, input);
    }

    #[test]
    fn apply_canonical_json_replaces_list_with_objects() {
        // Mutating an object inside a list triggers a full list rewrite.
        // The export should reflect the second apply exactly; the CRDT
        // op-log eats the cost (documented as a Phase 3 trade-off).
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({
            "items": [{ "id": 1 }, { "id": 2 }],
        }))
        .expect("first");
        let updated = serde_json::json!({
            "items": [{ "id": 1 }, { "id": 2, "label": "added" }],
        });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_replaces_scalar_with_list() {
        // Cross-type transition: previously-scalar key becomes a list.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "tags": "single" }))
            .expect("first");
        let updated = serde_json::json!({ "tags": ["a", "b"] });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_replaces_list_with_scalar() {
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "tags": ["a", "b"] }))
            .expect("first");
        let updated = serde_json::json!({ "tags": "single" });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    fn small_project_json() -> Value {
        // A minimal `json::Project`-shaped value: one model, two stocks,
        // one flow, one auxiliary, one module. The names are chosen so
        // canonicalization (lowercasing, space-to-underscore) is observable.
        serde_json::json!({
            "name": "demo",
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 10.0,
                "dt": "1",
                "method": "euler"
            },
            "models": [{
                "name": "main",
                "stocks": [
                    {
                        "name": "Population",
                        "initialEquation": "100",
                        "inflows": ["births"],
                        "outflows": []
                    },
                    {
                        "name": "Deaths Stock",
                        "initialEquation": "0",
                        "inflows": [],
                        "outflows": []
                    }
                ],
                "flows": [{
                    "name": "births",
                    "equation": "Population * 0.05",
                    "units": ""
                }],
                "auxiliaries": [{
                    "name": "Growth Rate",
                    "equation": "0.05",
                    "units": ""
                }],
                "modules": []
            }],
            "dimensions": [],
            "units": []
        })
    }

    #[test]
    fn project_json_round_trips_through_apply_export() {
        // Apply a small full project and assert export structurally
        // matches. The arrays come back sorted by `name` (alphabetic),
        // which is the canonical xmile order.
        let doc = ProjectDoc::new();
        let input = small_project_json();
        doc.apply_canonical_json(&input).expect("apply");
        let exported = doc.export_canonical_json().expect("export");

        // Top-level scalars and the simSpecs block must round-trip exactly.
        assert_eq!(exported["name"], input["name"]);
        assert_eq!(exported["simSpecs"], input["simSpecs"]);

        // The models array should still be an array (export inverted the
        // map projection) with one entry whose name is "main".
        let models = exported["models"].as_array().expect("models is array");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["name"], "main");

        // Stocks come back sorted by `name`. "Deaths Stock" comes before
        // "Population" alphabetically.
        let stocks = models[0]["stocks"].as_array().expect("stocks is array");
        assert_eq!(stocks.len(), 2);
        assert_eq!(stocks[0]["name"], "Deaths Stock");
        assert_eq!(stocks[1]["name"], "Population");

        // Flows / auxiliaries also preserved (only one each here).
        let flows = models[0]["flows"].as_array().expect("flows is array");
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0]["name"], "births");
        let auxes = models[0]["auxiliaries"]
            .as_array()
            .expect("auxiliaries is array");
        assert_eq!(auxes.len(), 1);
        assert_eq!(auxes[0]["name"], "Growth Rate");
    }

    #[test]
    fn variable_keys_are_canonical_form() {
        // The Loro storage keys are canonical (lowercase, spaces->underscores).
        // We verify by examining the doc's deep value directly: the key
        // for "Deaths Stock" must be the canonical form, not the raw form.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&small_project_json())
            .expect("apply");

        let inner = doc.inner_doc().get_deep_value();
        let json = loro_value_to_json(&inner);
        // Drill into project.models.main.stocks
        let stocks_map = &json["project"]["models"]["main"]["stocks"];
        // Canonical form: "deaths_stock" (lowercase, space -> underscore)
        assert!(
            stocks_map.get("deaths_stock").is_some(),
            "expected canonical key 'deaths_stock' in {stocks_map}"
        );
        assert!(
            stocks_map.get("population").is_some(),
            "expected canonical key 'population' in {stocks_map}"
        );
    }

    #[test]
    fn project_json_to_loro_shape_rejects_non_object_root() {
        let err = project_json_to_loro_shape(&serde_json::json!([1, 2, 3])).unwrap_err();
        match err {
            MergeError::ShapeError {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, "$");
                assert_eq!(expected, "object");
                assert_eq!(actual, "array");
            }
            other => panic!("expected ShapeError, got {other:?}"),
        }
    }

    #[test]
    fn project_json_to_loro_shape_rejects_non_array_models() {
        let bad = serde_json::json!({ "name": "x", "models": "not-an-array" });
        let err = project_json_to_loro_shape(&bad).unwrap_err();
        match err {
            MergeError::ShapeError { path, expected, .. } => {
                assert_eq!(path, "$.models");
                assert_eq!(expected, "array");
            }
            other => panic!("expected ShapeError, got {other:?}"),
        }
    }

    #[test]
    fn project_json_to_loro_shape_rejects_model_missing_name() {
        // A model object without a `name` field cannot be keyed; the
        // projection must surface a precise ShapeError pointing at the
        // bad element.
        let bad = serde_json::json!({
            "name": "x",
            "models": [{ "stocks": [] }]
        });
        let err = project_json_to_loro_shape(&bad).unwrap_err();
        match err {
            MergeError::ShapeError { path, .. } => {
                assert_eq!(path, "$.models[0].name");
            }
            other => panic!("expected ShapeError, got {other:?}"),
        }
    }

    #[test]
    fn project_json_to_loro_shape_rejects_variable_missing_name() {
        let bad = serde_json::json!({
            "name": "x",
            "models": [{
                "name": "main",
                "stocks": [{ "initialEquation": "0" }]
            }]
        });
        let err = project_json_to_loro_shape(&bad).unwrap_err();
        match err {
            MergeError::ShapeError { path, .. } => {
                assert_eq!(path, "$.models[0].stocks[0].name");
            }
            other => panic!("expected ShapeError, got {other:?}"),
        }
    }

    #[test]
    fn concurrent_serial_modify_different_stocks_preserves_both() {
        // The AC4.1 partial verification: simulate the production
        // execution model where two source-of-edit (browser tabs) each
        // produce a save modifying different stocks. Both saves are
        // sequenced through the registry write lock, so they apply
        // serially against the LoroDoc — but the per-stock map keying
        // means the *second* apply doesn't clobber the first stock's
        // edit even though a naive list-replace would.
        let doc = ProjectDoc::new();

        // Initial state: two stocks, S1 and S2, each with a placeholder
        // initial equation.
        let initial = serde_json::json!({
            "name": "demo",
            "simSpecs": {
                "startTime": 0.0, "endTime": 10.0, "dt": "1", "method": "euler"
            },
            "models": [{
                "name": "main",
                "stocks": [
                    { "name": "S1", "initialEquation": "0", "inflows": [], "outflows": [] },
                    { "name": "S2", "initialEquation": "0", "inflows": [], "outflows": [] }
                ]
            }]
        });
        doc.apply_canonical_json(&initial).expect("initial apply");

        // Save 1: client modifies only S1 (their canonical JSON still
        // contains both stocks but with S1's equation updated).
        let mut save_a = initial.clone();
        save_a["models"][0]["stocks"][0]["initialEquation"] = serde_json::json!("100");
        doc.apply_canonical_json(&save_a).expect("save A");

        // Save 2: client modifies only S2. Importantly, this client's
        // canonical JSON *also* carries the S1 baseline they last saw —
        // which still reads "100" because they refetched after save A,
        // mirroring the real-world flow. (The CRDT property is in the
        // *delta* shape: only S2 changed in this apply.)
        let mut save_b = save_a.clone();
        save_b["models"][0]["stocks"][1]["initialEquation"] = serde_json::json!("200");
        doc.apply_canonical_json(&save_b).expect("save B");

        let exported = doc.export_canonical_json().expect("export");
        let stocks = exported["models"][0]["stocks"]
            .as_array()
            .expect("stocks array");

        // Find both stocks by name (sort order is by name).
        let s1 = stocks
            .iter()
            .find(|v| v["name"] == "S1")
            .expect("S1 present");
        let s2 = stocks
            .iter()
            .find(|v| v["name"] == "S2")
            .expect("S2 present");
        assert_eq!(s1["initialEquation"], "100", "S1 edit preserved");
        assert_eq!(s2["initialEquation"], "200", "S2 edit preserved");
    }

    #[test]
    fn dropping_a_stock_removes_it_from_export() {
        // A save that drops a stock from the array should remove the
        // corresponding key from the LoroMap (since it's absent from the
        // incoming JSON). The export then reflects the smaller set.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&small_project_json())
            .expect("first apply");

        let mut updated = small_project_json();
        let stocks_arr = updated["models"][0]["stocks"].as_array_mut().unwrap();
        // Keep only "Population"; drop "Deaths Stock".
        stocks_arr.retain(|v| v["name"] == "Population");

        doc.apply_canonical_json(&updated).expect("second apply");
        let exported = doc.export_canonical_json().expect("export");
        let stocks = exported["models"][0]["stocks"]
            .as_array()
            .expect("stocks array");
        assert_eq!(stocks.len(), 1);
        assert_eq!(stocks[0]["name"], "Population");
    }

    #[test]
    fn loro_shape_to_canonical_json_sorts_models_by_name() {
        // Assemble a doc-shape value with models in non-alphabetic order
        // and verify the inverse projection sorts them.
        let shape = serde_json::json!({
            "name": "x",
            "models": {
                "zebra": { "name": "zebra" },
                "alpha": { "name": "alpha" }
            }
        });
        let out = loro_shape_to_canonical_json(shape).expect("invert");
        let models = out["models"].as_array().expect("models array");
        assert_eq!(models[0]["name"], "alpha");
        assert_eq!(models[1]["name"], "zebra");
    }

    #[test]
    fn loro_shape_to_canonical_json_sorts_variables_by_name() {
        // Variables should sort by their `name` field (case-sensitive),
        // matching the XMILE writer's order.
        let shape = serde_json::json!({
            "name": "x",
            "models": {
                "main": {
                    "name": "main",
                    "stocks": {
                        "zebra_stock": { "name": "Zebra Stock" },
                        "alpha_stock": { "name": "Alpha Stock" }
                    }
                }
            }
        });
        let out = loro_shape_to_canonical_json(shape).expect("invert");
        let stocks = out["models"][0]["stocks"].as_array().expect("stocks array");
        assert_eq!(stocks[0]["name"], "Alpha Stock");
        assert_eq!(stocks[1]["name"], "Zebra Stock");
    }

    #[test]
    fn apply_canonical_json_replaces_object_with_list() {
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "x": { "a": 1 } }))
            .expect("first");
        let updated = serde_json::json!({ "x": [1, 2] });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_replaces_list_with_object() {
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({ "x": [1] }))
            .expect("first");
        let updated = serde_json::json!({ "x": { "a": 1 } });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }

    #[test]
    fn apply_canonical_json_handles_deep_nested_object_to_scalar_transition() {
        // Three levels deep: the leaf transitions from an object to a scalar.
        let doc = ProjectDoc::new();
        doc.apply_canonical_json(&serde_json::json!({
            "a": { "b": { "c": { "leaf": true } } }
        }))
        .expect("first");
        let updated = serde_json::json!({
            "a": { "b": { "c": 42 } }
        });
        doc.apply_canonical_json(&updated).expect("second");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported, updated);
    }
}
