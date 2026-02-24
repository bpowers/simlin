// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! High-level model analysis API bundling compilation, simulation, and LTM loop discovery.
//!
//! This module provides `analyze_model()`, which takes a `datamodel::Project`,
//! runs the full LTM discovery pipeline, and returns a `ModelAnalysis` with
//! the model snapshot (views stripped), time array, per-loop importance series,
//! and dominant-period intervals.  On simulation failure the function returns
//! `Ok` with empty loop fields so callers can still display the model snapshot.

use crate::layout::metadata::{self, DominantPeriod, FeedbackLoop};
use crate::{datamodel, json};

/// Summary of a feedback loop discovered via LTM analysis.
pub struct LoopSummary {
    pub loop_id: String,
    pub name: Option<String>,
    pub polarity: String,
    pub variables: Vec<String>,
    pub importance: Vec<f64>,
}

/// Complete analysis results for a model.
pub struct ModelAnalysis {
    pub model: json::Model,
    pub time: Vec<f64>,
    pub loop_dominance: Vec<LoopSummary>,
    pub dominant_loops_by_period: Vec<DominantPeriod>,
}

/// Build a `json::Model` from the named model in a `datamodel::Project`, with
/// the `views` field cleared so the snapshot is diagram-free.
fn model_snapshot(project: &datamodel::Project, model_name: &str) -> json::Model {
    let model = project
        .get_model(model_name)
        .cloned()
        .or_else(|| project.models.first().cloned())
        .unwrap_or_else(|| datamodel::Model {
            name: model_name.to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        });

    let mut json_model: json::Model = model.into();
    json_model.views.clear();
    json_model
}

/// Analyse a system-dynamics model: run LTM discovery and return the
/// complete analysis bundle.
///
/// On any failure in the LTM pipeline (malformed equations, arrays, etc.)
/// `Ok` is returned with the model snapshot but empty loop fields, giving
/// callers graceful degradation rather than a hard error.
pub fn analyze_model(
    project: &datamodel::Project,
    model_name: &str,
) -> Result<ModelAnalysis, String> {
    let json_model = model_snapshot(project, model_name);

    let loop_result = run_ltm_pipeline(project, model_name);

    match loop_result {
        Some((time, loop_dominance, dominant_loops_by_period)) => Ok(ModelAnalysis {
            model: json_model,
            time,
            loop_dominance,
            dominant_loops_by_period,
        }),
        None => Ok(ModelAnalysis {
            model: json_model,
            time: vec![],
            loop_dominance: vec![],
            dominant_loops_by_period: vec![],
        }),
    }
}

/// Run the full LTM discovery pipeline.  Returns `None` on any failure.
fn run_ltm_pipeline(
    project: &datamodel::Project,
    model_name: &str,
) -> Option<(Vec<f64>, Vec<LoopSummary>, Vec<DominantPeriod>)> {
    use std::rc::Rc;

    // Canonicalize model name before moving project into catch_unwind.
    let actual_name = {
        let ident = project
            .models
            .iter()
            .find(|m| {
                crate::canonicalize(&m.name).as_ref() == crate::canonicalize(model_name).as_ref()
            })
            .map(|m| m.name.clone());
        ident.or_else(|| project.models.first().map(|m| m.name.clone()))?
    };

    // Build the UID -> loop name lookup from persisted loop_metadata before
    // the pipeline consumes the project.
    let uid_to_loop_name = build_uid_to_loop_name(project, &actual_name);

    let project_clone = project.clone();
    let actual_name_clone = actual_name.clone();

    // The LTM pipeline may panic on certain malformed models; catch_unwind
    // provides graceful degradation instead of crashing the server.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let compiled = crate::project::Project::from(project_clone);

        let ltm_project = compiled.with_ltm_all_links().ok()?;
        let ltm_project_rc = Rc::new(ltm_project);

        let sim = crate::interpreter::Simulation::new(&ltm_project_rc, &actual_name_clone).ok()?;
        let compiled_sim = sim.compile().ok()?;
        let mut vm = crate::vm::Vm::new(compiled_sim).ok()?;
        vm.run_to_end().ok()?;
        let results = vm.into_results();

        let found_loops = crate::ltm_finding::discover_loops(&results, &ltm_project_rc).ok()?;

        let time = build_time_array(&results);

        let feedback_loops: Vec<FeedbackLoop> = found_loops.iter().map(to_feedback_loop).collect();

        let dominant_loops_by_period = metadata::calculate_dominant_periods(
            &feedback_loops,
            results.specs.start,
            results.specs.save_step,
        );

        let loop_dominance: Vec<LoopSummary> = found_loops
            .iter()
            .map(|fl| to_loop_summary(fl, &uid_to_loop_name, &actual_name_clone, &ltm_project_rc))
            .collect();

        Some((time, loop_dominance, dominant_loops_by_period))
    }));
    if let Err(ref panic) = result {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        eprintln!("simlin-engine: LTM pipeline panicked: {msg}");
    }
    result.ok().flatten()
}

/// Build a mapping from variable-UID sets to loop names using the model's
/// persisted `loop_metadata`.  Entries are stored as a sorted Vec of UIDs
/// because UIDs must be matched as a set.
fn build_uid_to_loop_name(
    project: &datamodel::Project,
    model_name: &str,
) -> Vec<(Vec<i32>, String)> {
    let Some(model) = project.get_model(model_name) else {
        return vec![];
    };
    model
        .loop_metadata
        .iter()
        .filter(|lm| !lm.deleted && !lm.name.is_empty())
        .map(|lm| {
            let mut uids = lm.uids.clone();
            uids.sort_unstable();
            (uids, lm.name.clone())
        })
        .collect()
}

/// Convert a `FoundLoop` to the `FeedbackLoop` form expected by
/// `calculate_dominant_periods`.
fn to_feedback_loop(fl: &crate::ltm_finding::FoundLoop) -> FeedbackLoop {
    let polarity = match fl.loop_info.polarity {
        crate::ltm::LoopPolarity::Reinforcing => metadata::LoopPolarity::Reinforcing,
        crate::ltm::LoopPolarity::Balancing => metadata::LoopPolarity::Balancing,
        crate::ltm::LoopPolarity::Undetermined => metadata::LoopPolarity::Undetermined,
    };

    let variables = loop_variables(fl);

    let importance_series: Vec<f64> = fl
        .scores
        .iter()
        .map(|(_, s)| if s.is_finite() { *s } else { 0.0 })
        .collect();

    FeedbackLoop {
        name: fl.loop_info.id.clone(),
        polarity,
        variables,
        importance_series,
        dominant_period: None,
    }
}

/// Extract the ordered variable names from a `FoundLoop`.
fn loop_variables(fl: &crate::ltm_finding::FoundLoop) -> Vec<String> {
    let mut vars: Vec<String> = fl
        .loop_info
        .links
        .iter()
        .map(|l| l.from.to_string())
        .collect();
    if let Some(first) = vars.first().cloned() {
        vars.push(first);
    }
    vars
}

/// Convert a `FoundLoop` to a `LoopSummary`, resolving a human-readable
/// name from persisted `loop_metadata` when the UID sets match.
fn to_loop_summary(
    fl: &crate::ltm_finding::FoundLoop,
    uid_to_loop_name: &[(Vec<i32>, String)],
    model_name: &str,
    project: &crate::project::Project,
) -> LoopSummary {
    let polarity = match fl.loop_info.polarity {
        crate::ltm::LoopPolarity::Reinforcing => "reinforcing",
        crate::ltm::LoopPolarity::Balancing => "balancing",
        crate::ltm::LoopPolarity::Undetermined => "undetermined",
    }
    .to_string();

    let variables = loop_variables(fl);

    let importance: Vec<f64> = fl
        .scores
        .iter()
        .map(|(_, s)| if s.is_finite() { s.abs() } else { 0.0 })
        .collect();

    let name = resolve_loop_name(fl, uid_to_loop_name, model_name, project);

    LoopSummary {
        loop_id: fl.loop_info.id.clone(),
        name,
        polarity,
        variables,
        importance,
    }
}

/// Try to match loop variables to a persisted loop name via UID sets.
///
/// UID lookup is scoped to `model_name` so that variables with the same
/// identifier in different models don't produce false matches.
fn resolve_loop_name(
    fl: &crate::ltm_finding::FoundLoop,
    uid_to_loop_name: &[(Vec<i32>, String)],
    model_name: &str,
    project: &crate::project::Project,
) -> Option<String> {
    if uid_to_loop_name.is_empty() {
        return None;
    }

    // Collect UIDs for variables in the loop from the compiled project,
    // restricted to the model being analyzed.
    let loop_var_idents: std::collections::HashSet<String> = fl
        .loop_info
        .links
        .iter()
        .map(|l| l.from.to_string())
        .collect();

    let mut loop_uids: Vec<i32> = project
        .datamodel
        .models
        .iter()
        .filter(|m| {
            crate::canonicalize(&m.name).as_ref() == crate::canonicalize(model_name).as_ref()
        })
        .flat_map(|m| &m.variables)
        .filter_map(|var| {
            let ident = crate::canonicalize(match var {
                datamodel::Variable::Stock(s) => &s.ident,
                datamodel::Variable::Flow(f) => &f.ident,
                datamodel::Variable::Aux(a) => &a.ident,
                datamodel::Variable::Module(m) => &m.ident,
            });
            let uid = match var {
                datamodel::Variable::Stock(s) => s.uid,
                datamodel::Variable::Flow(f) => f.uid,
                datamodel::Variable::Aux(a) => a.uid,
                datamodel::Variable::Module(m) => m.uid,
            };
            if loop_var_idents.contains(ident.as_ref()) {
                uid
            } else {
                None
            }
        })
        .collect();
    loop_uids.sort_unstable();

    uid_to_loop_name
        .iter()
        .find(|(uids, _)| *uids == loop_uids)
        .map(|(_, name)| name.clone())
}

/// Build the time array from simulation results specs.
fn build_time_array(results: &crate::results::Results) -> Vec<f64> {
    (0..results.step_count)
        .map(|i| results.specs.start + (i as f64) * results.specs.save_step)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    fn load_project(path: &str) -> datamodel::Project {
        use std::fs::File;
        use std::io::BufReader;
        let f = File::open(path).expect("failed to open test fixture");
        let mut reader = BufReader::new(f);
        crate::xmile::project_from_reader(&mut reader).expect("failed to parse XMILE")
    }

    fn broken_project() -> datamodel::Project {
        crate::test_common::TestProject::new("broken")
            .stock("population", "10", &["births"], &[], None)
            .flow("births", "nonexistent_variable * population", None)
            .build_datamodel()
    }

    // ---- AC2.1: loop_dominance and dominant_loops_by_period are non-empty ----

    #[test]
    fn ac2_1_logistic_growth_returns_loops() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let analysis = analyze_model(&project, "main").expect("analyze_model failed");

        assert!(
            !analysis.loop_dominance.is_empty(),
            "expected non-empty loop_dominance, got none"
        );
        assert!(
            !analysis.dominant_loops_by_period.is_empty(),
            "expected non-empty dominant_loops_by_period, got none"
        );
    }

    // ---- AC2.2: time array length, non-empty importance, valid period bounds ----

    #[test]
    fn ac2_2_time_array_and_importance_consistency() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let analysis = analyze_model(&project, "main").expect("analyze_model failed");

        // Time array must be non-empty and its length must match loop importance lengths.
        assert!(!analysis.time.is_empty(), "time array must not be empty");

        for summary in &analysis.loop_dominance {
            assert!(
                !summary.importance.is_empty(),
                "loop {} must have a non-empty importance series",
                summary.loop_id
            );
            assert_eq!(
                summary.importance.len(),
                analysis.time.len(),
                "importance series length ({}) must equal time array length ({})",
                summary.importance.len(),
                analysis.time.len()
            );
        }

        // Dominant period bounds must lie within [start, stop].
        if let Some(first_time) = analysis.time.first() {
            let last_time = analysis.time.last().copied().unwrap_or(*first_time);
            for period in &analysis.dominant_loops_by_period {
                assert!(
                    period.start >= *first_time,
                    "period start {} is before simulation start {}",
                    period.start,
                    first_time
                );
                assert!(
                    period.end <= last_time,
                    "period end {} is after simulation end {}",
                    period.end,
                    last_time
                );
            }
        }
    }

    // ---- AC2.5: model snapshot has empty views ----

    #[test]
    fn ac2_5_views_are_empty() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let analysis = analyze_model(&project, "main").expect("analyze_model failed");

        assert!(
            analysis.model.views.is_empty(),
            "model snapshot must have empty views, got {}",
            analysis.model.views.len()
        );
    }

    // ---- AC2.6: graceful degradation on equation errors ----

    #[test]
    fn ac2_6_broken_model_returns_empty_loops() {
        let project = broken_project();
        let analysis =
            analyze_model(&project, "main").expect("analyze_model should not return Err");

        // Model snapshot must still be present with the stock and flow.
        assert!(
            !analysis.model.stocks.is_empty() || !analysis.model.flows.is_empty(),
            "model snapshot must contain variables even on simulation failure"
        );

        // Loop-related fields must be empty.
        assert!(
            analysis.time.is_empty(),
            "time must be empty when simulation fails"
        );
        assert!(
            analysis.loop_dominance.is_empty(),
            "loop_dominance must be empty when simulation fails"
        );
        assert!(
            analysis.dominant_loops_by_period.is_empty(),
            "dominant_loops_by_period must be empty when simulation fails"
        );
    }
}
