// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Lightweight model topology used by unit analysis.
//!
//! Equations are parsed and lowered per variable. This module retains only the
//! names needed to walk the models reachable through explicit, stdlib, and
//! project-macro module instances; no query here owns an equation AST.

use std::collections::{BTreeMap, BTreeSet};

use crate::common::{Canonical, Ident};
use crate::db::{Db, SourceModel, SourceProject};

fn model_is_stdlib(canonical_model_name: &str) -> bool {
    canonical_model_name
        .strip_prefix("stdlib\u{205A}")
        .is_some_and(|suffix| crate::stdlib::MODEL_NAMES.contains(&suffix))
}

/// Whether a salsa model handle denotes one of the built-in stdlib templates.
pub(crate) fn source_model_is_stdlib(db: &dyn Db, model: SourceModel) -> bool {
    model_is_stdlib(Ident::<Canonical>::new(model.name(db)).as_str())
}

/// Module target and instance-name candidates declared by one model.
///
/// Explicit instances come straight from their salsa inputs. Parse-synthesized
/// stdlib and macro instances come from the same implicit metadata consumed by
/// layout and fragment compilation. The result contains names only, so an
/// equation edit that does not change module topology backdates here.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_module_targets(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeSet<Ident<Canonical>> {
    let mut targets = BTreeSet::new();
    for source_var in model.variables(db).values() {
        if source_var.kind(db) == crate::db::SourceVariableKind::Module {
            targets.insert(Ident::new(source_var.model_name(db)));
            targets.insert(Ident::new(source_var.ident(db)));
        }
    }
    for meta in crate::db::model_implicit_var_info(db, model, project).values() {
        if !meta.is_module {
            continue;
        }
        if let Some(model_name) = &meta.model_name {
            targets.insert(Ident::new(model_name));
        }
        targets.insert(Ident::new(&meta.name));
    }
    targets
}

/// The model plus the transitive module targets its unit constraints can reach.
///
/// The iterative worklist handles user-authored module cycles without invoking
/// recursive salsa queries. Both target model names and instance names are
/// considered because imported module wiring can still use the historical
/// `instance name == model name` spelling during source validation.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_scope_models(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeMap<Ident<Canonical>, SourceModel> {
    let project_models = project.models(db);
    let root_ident: Ident<Canonical> = Ident::new(model.name(db));
    let mut scope = BTreeMap::new();
    if let Some(source) = project_models.get(root_ident.as_str()) {
        scope.insert(root_ident.clone(), *source);
    }

    let mut visited: BTreeSet<Ident<Canonical>> = [root_ident].into_iter().collect();
    let mut queue = vec![model];
    while let Some(source) = queue.pop() {
        for target in model_module_targets(db, source, project) {
            if !visited.insert(target.clone()) {
                continue;
            }
            if let Some(next) = project_models.get(target.as_str()) {
                scope.insert(target.clone(), *next);
                queue.push(*next);
            }
        }
    }
    scope
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    fn scope_names(db: &SimlinDb, sync: &crate::db::SyncResult, model: &str) -> Vec<String> {
        model_scope_models(db, sync.models[model].source, sync.project)
            .keys()
            .map(|ident| ident.as_str().to_string())
            .collect()
    }

    #[test]
    fn stdlib_detection_requires_a_real_template_name() {
        assert!(model_is_stdlib("stdlib\u{205a}smth1"));
        assert!(!model_is_stdlib("stdlib\u{205a}not_a_template"));
        assert!(!model_is_stdlib("smth1"));
    }

    #[test]
    fn source_model_stdlib_detection_canonicalizes_the_synced_display_name() {
        let db = SimlinDb::default();
        let project = x_project(
            sim_specs_with_units("month"),
            &[
                x_model("main", vec![x_aux("x", "1", None)]),
                x_model("Stdlib\u{205a}Smth1", vec![x_aux("input", "1", None)]),
            ],
        );
        let sync = sync_from_datamodel(&db, &project);
        let shadow = sync.models["stdlib\u{205a}smth1"].source;

        assert_eq!(
            shadow.name(&db),
            "Stdlib\u{205a}Smth1",
            "sync retains the raw display name on SourceModel"
        );
        assert!(
            source_model_is_stdlib(&db, shadow),
            "the production classifier must canonicalize the SourceModel name"
        );
    }

    #[test]
    fn every_spliced_stdlib_is_classified_and_is_a_module_sink() {
        let db = SimlinDb::default();
        let project = x_project(
            sim_specs_with_units("month"),
            &[x_model("main", vec![x_aux("x", "1", None)])],
        );
        let sync = sync_from_datamodel(&db, &project);
        let mut seen = 0;
        for (canonical_name, source) in sync.project.models(&db) {
            if !canonical_name.starts_with("stdlib\u{205a}") {
                continue;
            }
            seen += 1;
            assert!(
                source_model_is_stdlib(&db, *source),
                "spliced template {canonical_name} must satisfy the strict classifier"
            );
            assert!(
                model_module_targets(&db, *source, sync.project).is_empty(),
                "stdlib template {canonical_name} must remain a module sink"
            );
        }
        assert_eq!(seen, crate::stdlib::MODEL_NAMES.len());
    }

    #[test]
    fn explicit_target_and_instance_alias_edges_form_a_transitive_closure() {
        let db = SimlinDb::default();
        let project = x_project(
            sim_specs_with_units("month"),
            &[
                x_model("main", vec![x_module_named("alias", "target", &[], None)]),
                x_model("target", vec![x_aux("out", "1", None)]),
                x_model("alias", vec![x_module_named("nested", "leaf", &[], None)]),
                x_model("leaf", vec![x_aux("out", "2", None)]),
            ],
        );
        let sync = sync_from_datamodel(&db, &project);
        assert_eq!(
            scope_names(&db, &sync, "main"),
            ["alias", "leaf", "main", "target"],
            "both explicit target and legacy instance-name edges must be followed transitively"
        );
    }

    #[test]
    fn a_module_cycle_produces_a_finite_scope() {
        let db = SimlinDb::default();
        let project = x_project(
            sim_specs_with_units("month"),
            &[
                x_model("a", vec![x_module_named("to_b", "b", &[], None)]),
                x_model("b", vec![x_module_named("to_a", "a", &[], None)]),
            ],
        );
        let sync = sync_from_datamodel(&db, &project);
        assert_eq!(scope_names(&db, &sync, "a"), ["a", "b"]);
        assert_eq!(scope_names(&db, &sync, "b"), ["a", "b"]);
    }
}
