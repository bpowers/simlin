// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// `compute_diagnostic_set` is a pure (Functional Core) helper, but
// `maybe_emit_diagnostics_changed` orchestrates the registry write lock
// and EventBus broadcast and so classifies the whole module as Shell.
// The module stays small enough to keep both surfaces co-located rather
// than splitting along pure/impure lines.

//! Diagnostic-set computation and the merge-path broadcast helper.
//!
//! `compute_diagnostic_set` runs the engine's salsa-based diagnostic
//! pipeline once and returns both:
//! - a `BTreeSet<(code, variable_name)>` — the cheap comparison key the
//!   registry caches on each `ProjectMeta`.
//! - a `Vec<ValidationError>` — the formatted error list ready to ship as
//!   the payload of `WsMessage::DiagnosticsChanged`.
//!
//! `maybe_emit_diagnostics_changed` is the merge-path helper invoked
//! from each of the four surfaces that produce a successful merge (the
//! HTTP save handler, the MCP `RegistryAccess::save` and `::create`
//! paths, and the file watcher). It computes the new set, drives the
//! atomic compare-and-update on the registry
//! (`ProjectRegistry::update_diagnostic_keys_if_changed`, which owns the
//! "did the set actually change?" decision so the comparison and the
//! cache write observe the same snapshot), and publishes the
//! notification when the set differs.

use std::collections::BTreeSet;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_engine::db::{
    DiagnosticSeverity, LtmOverlay, SimlinDb, collect_all_diagnostics, sync_from_datamodel,
};
use simlin_engine::errors::{FormattedErrorKind, collect_formatted_errors};

use crate::events::{ValidationError, WsMessage};
use crate::handlers::AppState;
use crate::path_resolution::to_forward_slash;

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
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
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

/// Recompute diagnostics for `project`, atomically swap the cached set
/// on the registry entry keyed by `abs_path`, and (only when the set
/// actually changed) broadcast `WsMessage::DiagnosticsChanged` on the
/// shared `EventBus`.
///
/// Ordering invariant: each merge surface MUST publish its
/// `ProjectChanged` notification BEFORE calling this helper. Both
/// publishes happen sequentially inside the same async task, and
/// `tokio::sync::broadcast` preserves FIFO order within one sender's
/// call sequence, so subscribers always observe `ProjectChanged`
/// followed by `DiagnosticsChanged`. A future maintainer who is
/// tempted to parallelize the two publishes would break this contract;
/// the per-session MCP forwarder (Phase 7 Subcomponent D) and the
/// browser editor both rely on it to avoid showing diagnostics for a
/// version the consumer has not yet observed.
///
/// The registry write lock is held only across the compare-and-swap on
/// `last_diagnostic_keys`. Lock duration scales with the cost of one
/// `BTreeSet` clone — at most a handful of small `(String, Option<String>)`
/// pairs — and is independent of the diagnostic-pipeline cost (which
/// runs lock-free before the lock is taken). The lock is dropped before
/// the broadcast so a subscriber doing extra work in its `recv` path
/// cannot stall other registry mutators.
///
/// `abs_path` is the canonical absolute path used as the registry key.
/// The wire `path` field is computed by stripping the registry root
/// prefix and rendering through [`to_forward_slash`] so the wire shape
/// matches what `ProjectChanged` would publish for the same operation.
/// The fallback when the prefix doesn't apply is the absolute path,
/// which would be a programming error the caller should surface
/// elsewhere; it exists so the notification still carries *something*
/// useful for debugging.
pub fn maybe_emit_diagnostics_changed(
    state: &AppState,
    abs_path: &Path,
    project: &datamodel::Project,
) {
    let (new_keys, formatted) = compute_diagnostic_set(project);

    if !state
        .registry
        .update_diagnostic_keys_if_changed(abs_path, &new_keys)
    {
        return;
    }

    let rel = abs_path
        .strip_prefix(state.root.as_ref())
        .unwrap_or(abs_path);

    state.events.publish(WsMessage::DiagnosticsChanged {
        path: to_forward_slash(rel),
        errors: formatted,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use simlin_engine::json;

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
}
