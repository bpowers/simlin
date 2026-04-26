// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Pure helpers that compute a project's current diagnostic set and decide
// whether it differs from the cached set on a registry entry. The save
// handler, the watcher merge path, and the MCP RegistryAccess path all
// drive the same comparison through these helpers, so the wire shape of
// `DiagnosticsChanged` is canonical regardless of which surface produced
// the change.

//! Diagnostic-set computation and change detection.
//!
//! `compute_diagnostic_set` runs the engine's salsa-based diagnostic
//! pipeline once and returns both:
//! - a `BTreeSet<(code, variable_name)>` — the cheap comparison key the
//!   registry caches on each `ProjectMeta`.
//! - a `Vec<ValidationError>` — the formatted error list ready to ship as
//!   the payload of `WsMessage::DiagnosticsChanged`.
//!
//! `diagnostics_set_changed` compares a `ProjectMeta`'s cached
//! `last_diagnostic_keys` against a freshly computed set; the call sites
//! emit a `DiagnosticsChanged` notification only when the two differ.

use std::collections::BTreeSet;

use simlin_engine::datamodel;
use simlin_engine::db::{
    DiagnosticSeverity, SimlinDb, collect_all_diagnostics, sync_from_datamodel,
};
use simlin_engine::errors::{FormattedErrorKind, collect_formatted_errors};

use crate::events::ValidationError;
use crate::registry::ProjectMeta;

/// Canonical ordered key for one validation diagnostic. Pair of (error
/// code, optional variable name). Used as the comparison key for
/// detecting "did the diagnostic set actually change since last save".
///
/// `BTreeSet` rather than `HashSet` so the comparison is order-stable
/// and equality is `O(n)` without rehashing — both sets are typically
/// small (single-digit), so the constant-factor difference is irrelevant
/// next to the readability win.
pub type DiagnosticKey = (String, Option<String>);

/// Run the engine diagnostic pipeline for `project` and return both the
/// canonical key set and the formatted error list.
///
/// Both outputs are derived from the same single pipeline invocation:
/// the keys feed the registry's "did the set change?" cache, the
/// formatted errors feed the wire payload of `DiagnosticsChanged`.
/// Callers that need only one side should still go through here so the
/// two stay strictly in sync.
///
/// Side-effect free: the engine's `SimlinDb` is constructed locally and
/// dropped on return, so this is safe to call from any thread without
/// regard to concurrency.
pub fn compute_diagnostic_set(
    project: &datamodel::Project,
) -> (BTreeSet<DiagnosticKey>, Vec<ValidationError>) {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    let diagnostics = collect_all_diagnostics(&db, &sync);
    let formatted = collect_formatted_errors(
        diagnostics
            .iter()
            .filter(|d| matches!(d.severity, DiagnosticSeverity::Error)),
        project,
    );

    let errors: Vec<ValidationError> = formatted
        .errors
        .into_iter()
        .map(|fe| {
            let kind = match fe.kind {
                FormattedErrorKind::Project => "project",
                FormattedErrorKind::Model => "model",
                FormattedErrorKind::Variable => "variable",
                FormattedErrorKind::Units => "units",
                FormattedErrorKind::Simulation => "simulation",
            };
            ValidationError {
                code: fe.code.to_string(),
                message: fe.message.unwrap_or_default(),
                model_name: fe.model_name,
                variable_name: fe.variable_name,
                kind: kind.to_string(),
            }
        })
        .collect();

    let keys: BTreeSet<DiagnosticKey> = errors
        .iter()
        .map(|e| (e.code.clone(), e.variable_name.clone()))
        .collect();

    (keys, errors)
}

/// True iff `new_keys` differs from `meta.last_diagnostic_keys`. The
/// helper is a one-line `BTreeSet` equality, but exists so call sites
/// can read at a glance and so a future replacement (e.g. excluding
/// transient warning codes) lives in one place.
pub fn diagnostics_set_changed(meta: &ProjectMeta, new_keys: &BTreeSet<DiagnosticKey>) -> bool {
    meta.last_diagnostic_keys != *new_keys
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::registry::{GitState, ProjectFormat, ProjectMeta};
    use simlin_engine::json;
    use std::path::PathBuf;
    use std::time::SystemTime;

    /// Minimal valid project: one model, no variables. Should produce no
    /// error diagnostics.
    const EMPTY_VALID: &str = r#"{
        "name": "demo",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{"name": "main"}]
    }"#;

    /// A project that references an undefined identifier — guaranteed
    /// to produce an `unknown_dependency` diagnostic on the auxiliary
    /// `bad`.
    const HAS_UNDEFINED_REF: &str = r#"{
        "name": "demo",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{
            "name": "main",
            "auxiliaries": [
                {"name": "bad", "equation": "1 + bogus"}
            ]
        }]
    }"#;

    fn project_from_json(body: &str) -> datamodel::Project {
        let json_project: json::Project = serde_json::from_str(body).expect("test fixture parses");
        json_project.into()
    }

    fn meta_with_keys(keys: BTreeSet<DiagnosticKey>) -> ProjectMeta {
        ProjectMeta {
            path: PathBuf::new(),
            format: ProjectFormat::SdJson,
            mtime: SystemTime::UNIX_EPOCH,
            size: 0,
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
            last_diagnostic_keys: keys,
        }
    }

    #[test]
    fn clean_project_yields_empty_set_and_empty_errors() {
        let project = project_from_json(EMPTY_VALID);
        let (keys, errors) = compute_diagnostic_set(&project);
        assert!(
            keys.is_empty(),
            "clean project must have an empty key set, got {keys:?}"
        );
        assert!(
            errors.is_empty(),
            "clean project must have no formatted errors, got {errors:?}"
        );
    }

    #[test]
    fn broken_project_surfaces_undefined_reference_in_set_and_errors() {
        let project = project_from_json(HAS_UNDEFINED_REF);
        let (keys, errors) = compute_diagnostic_set(&project);

        assert!(
            !keys.is_empty(),
            "broken project must have at least one key entry"
        );
        assert!(
            keys.iter()
                .any(|(code, var)| code == "unknown_dependency" && var.as_deref() == Some("bad")),
            "expected (unknown_dependency, Some(\"bad\")) in keys, got {keys:?}"
        );

        // Same diagnostic must show up in the formatted error list with
        // matching code + variable.
        let bad = errors
            .iter()
            .find(|e| e.variable_name.as_deref() == Some("bad"))
            .expect("error for variable 'bad' present");
        assert_eq!(bad.code, "unknown_dependency");
    }

    #[test]
    fn keys_and_errors_are_consistent() {
        // For every formatted error the (code, variable_name) pair must
        // appear in `keys`. The reverse holds because keys is built
        // directly from errors; this test guards against a future
        // refactor accidentally feeding the two sides from different
        // diagnostic passes.
        let project = project_from_json(HAS_UNDEFINED_REF);
        let (keys, errors) = compute_diagnostic_set(&project);
        for err in &errors {
            let key = (err.code.clone(), err.variable_name.clone());
            assert!(
                keys.contains(&key),
                "every formatted error must have its key in the set; missing {key:?}"
            );
        }
        assert_eq!(
            keys.len(),
            errors.len(),
            "key set and error list should have matching cardinality for unique keys"
        );
    }

    #[test]
    fn diagnostics_set_changed_returns_false_for_equal_sets() {
        let mut keys = BTreeSet::new();
        keys.insert(("syntax".to_string(), Some("x".to_string())));
        let meta = meta_with_keys(keys.clone());
        assert!(
            !diagnostics_set_changed(&meta, &keys),
            "equal sets must report unchanged"
        );
    }

    #[test]
    fn diagnostics_set_changed_returns_true_when_new_key_appears() {
        let baseline = BTreeSet::new();
        let meta = meta_with_keys(baseline);

        let mut after_edit = BTreeSet::new();
        after_edit.insert(("unknown_dependency".to_string(), Some("bad".to_string())));
        assert!(
            diagnostics_set_changed(&meta, &after_edit),
            "introducing a new error must report changed"
        );
    }

    #[test]
    fn diagnostics_set_changed_returns_true_when_keys_disappear() {
        // The "fixed all errors" path: meta has cached error keys, the
        // recomputed set is empty.
        let mut cached = BTreeSet::new();
        cached.insert(("syntax".to_string(), Some("x".to_string())));
        let meta = meta_with_keys(cached);

        let after_fix: BTreeSet<DiagnosticKey> = BTreeSet::new();
        assert!(
            diagnostics_set_changed(&meta, &after_fix),
            "transitioning to no errors must report changed"
        );
    }

    #[test]
    fn diagnostics_set_changed_returns_true_when_variable_differs() {
        // Same code, different variable name: still a different set
        // entry. Catches a regression where the comparison drops the
        // variable and only inspects the code.
        let mut cached = BTreeSet::new();
        cached.insert(("syntax".to_string(), Some("x".to_string())));
        let meta = meta_with_keys(cached);

        let mut after_edit = BTreeSet::new();
        after_edit.insert(("syntax".to_string(), Some("y".to_string())));
        assert!(
            diagnostics_set_changed(&meta, &after_edit),
            "different variable_name must produce a changed set"
        );
    }

    #[test]
    fn introducing_an_error_is_observable_through_full_pipeline() {
        // Wire-level integration: clean project → empty set → a meta with
        // empty keys reports unchanged. Then a fresh compute on a broken
        // project produces a non-empty set; the same meta reports
        // changed. This is the exact sequence the save handler and
        // watcher invoke.
        let clean = project_from_json(EMPTY_VALID);
        let (clean_keys, _) = compute_diagnostic_set(&clean);
        let meta = meta_with_keys(clean_keys.clone());
        assert!(!diagnostics_set_changed(&meta, &clean_keys));

        let broken = project_from_json(HAS_UNDEFINED_REF);
        let (broken_keys, _) = compute_diagnostic_set(&broken);
        assert!(diagnostics_set_changed(&meta, &broken_keys));
    }
}
