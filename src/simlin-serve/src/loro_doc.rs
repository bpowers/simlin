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
#[allow(dead_code)] // wired up in Task 2's apply_canonical_json
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
    /// Single `commit()` per call so subscribe_root callbacks fire once.
    ///
    /// Phase 3 stub: real implementation lands in Tasks 2-4.
    pub fn apply_canonical_json(&self, _new_json: &Value) -> Result<(), MergeError> {
        // Stub: Tasks 2-4 implement scalar/nested-map/list/project-shape merge.
        // The commit at the end of the real impl is what lets subscribers
        // observe the full op batch as one logical change.
        Ok(())
    }

    /// Inverse of `apply_canonical_json`: snapshot the doc's current deep
    /// value as a `serde_json::Value`. Tasks 2-4 progressively round-trip
    /// scalar/nested-map/list/project-shape; for now we expose the raw
    /// LoroValue->serde_json conversion so the wrapper's plumbing is
    /// testable.
    pub fn export_canonical_json(&self) -> Result<Value, MergeError> {
        let loro_value = self.doc.get_deep_value();
        Ok(loro_value_to_json(&loro_value))
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

/// Suppress unused-warnings while Tasks 2-4 are landing. Once the merge
/// primitive uses these helpers they'll be wired in directly.
#[allow(dead_code)]
fn _phase3_task1_helpers_keepalive(
    map: &LoroMap,
    list: &LoroList,
    json: &JsonMap<String, Value>,
    val: &Value,
    voc: ValueOrContainer,
    _container: Container,
) -> Result<HashSet<String>, MergeError> {
    let _ = (map, list, json, val, voc);
    let _ = json_to_loro_value;
    Ok(HashSet::new())
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
        // Bypass apply_canonical_json (still a stub in Task 1) and write
        // directly through the inner LoroDoc to prove the export plumbing
        // works end-to-end before we tackle the diff logic in Tasks 2-4.
        let doc = ProjectDoc::new();
        let root = doc.inner_doc().get_map(ROOT_MAP_KEY);
        root.insert("name", "demo")
            .expect("insert string into root map");
        doc.inner_doc().commit();

        let exported = doc.export_canonical_json().expect("export");
        // The doc now has one root container `project` with key "name".
        let project = exported
            .as_object()
            .and_then(|m| m.get(ROOT_MAP_KEY))
            .and_then(|v| v.as_object())
            .expect("project map present");
        assert_eq!(project.get("name").and_then(|v| v.as_str()), Some("demo"));
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
}
