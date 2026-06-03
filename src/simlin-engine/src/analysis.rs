// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! High-level model analysis API bundling compilation, simulation, and LTM loop discovery.
//!
//! This module provides `analyze_model()`, which takes a `datamodel::Project`
//! along with a `SimlinDb` and `SourceProject` for incremental compilation,
//! runs the full LTM discovery pipeline, and returns a `ModelAnalysis` with
//! the model snapshot (views stripped), time array, per-loop importance series,
//! and dominant-period intervals.  On simulation failure the function returns
//! `Ok` with empty loop fields so callers can still display the model snapshot.

use crate::db::{SimlinDb, SourceProject};
use crate::layout::metadata::{self, DominantPeriod, FeedbackLoop};
use crate::{datamodel, json};

/// Summary of a feedback loop discovered via LTM analysis.
pub struct LoopSummary {
    pub loop_id: String,
    pub name: Option<String>,
    pub polarity: String,
    pub variables: Vec<String>,
    /// Per-timestep SIGNED partition-relative loop score, a value in `[-1, 1]`:
    /// the loop's score divided by the sum of `|loop_score|` over the loops
    /// sharing its cycle partition (sign preserved; non-finite coerced to 0).
    /// This is comparable across partitions and consistent with the engine's
    /// mean-relative loop ranking and the pinned-loop relative-score path --
    /// unlike a raw `|loop score|`, which is partition-incomparable (GH #543).
    pub importance: Vec<f64>,
    /// RESULT-SCOPED index into [`ModelAnalysis::partitions`] identifying the
    /// loop's cycle partition, or `None` for a loop with no parent-level
    /// partition (a pure module-internal loop).  See
    /// `ltm_finding::FoundLoop::partition` for the stability caveats (indices
    /// are dense per result, not durable across runs/edits).
    pub partition: Option<usize>,
}

/// Complete analysis results for a model.
pub struct ModelAnalysis {
    pub model: json::Model,
    pub time: Vec<f64>,
    pub loop_dominance: Vec<LoopSummary>,
    /// The cycle partitions referenced by `loop_dominance` (indexed by each
    /// summary's `partition`): a partition's element-level stock names plus
    /// how many returned loops belong to it.  Loops compete for dominance only
    /// WITHIN a partition (relative scores are normalized per partition), so
    /// this is the grouping callers need to present loops
    /// partition-by-partition.
    pub partitions: Vec<crate::ltm_finding::DiscoveredPartition>,
    pub dominant_loops_by_period: Vec<DominantPeriod>,
    /// True when loop discovery hit its time budget before finishing, so the
    /// loop fields may be partial. See `discover_loops_with_graph`'s budget.
    pub truncated: bool,
    /// True when discovery's cross-element-through-aggregate loop recovery
    /// (GH #696) hit its loop-count budget, so some cross-agg reducer loops are
    /// absent from `loop_dominance`. Distinct from `truncated` (which is the
    /// wall-clock time budget); this is the structural-completeness signal that
    /// mirrors exhaustive mode's `LtmVariablesResult::agg_recovery_truncated`.
    pub agg_recovery_truncated: bool,
}

/// Build a `json::Model` from the named model in a `datamodel::Project`, with
/// the `views` field cleared so the snapshot is diagram-free.
///
/// Returns `None` when the requested model name is not found and is not the
/// "main" convenience alias.
fn model_snapshot(project: &datamodel::Project, model_name: &str) -> Option<json::Model> {
    let model = if model_name == "main" {
        // "main" is a convenience alias: fall back to first model when no model
        // is explicitly named "main".
        project
            .get_model(model_name)
            .cloned()
            .or_else(|| project.models.first().cloned())
    } else {
        project.get_model(model_name).cloned()
    }?;

    let mut json_model: json::Model = model.into();
    json_model.views.clear();
    Some(json_model)
}

/// Analyse a system-dynamics model: run LTM discovery and return the
/// complete analysis bundle.
///
/// The caller provides a salsa `SimlinDb` and `SourceProject` (already
/// synced from the datamodel project) for incremental compilation, plus
/// the `datamodel::Project` for model snapshot construction and UID
/// resolution.
///
/// On any failure in the LTM pipeline (malformed equations, arrays, etc.)
/// `Ok` is returned with the model snapshot but empty loop fields, giving
/// callers graceful degradation rather than a hard error.
///
/// `budget` optionally bounds the wall-clock time spent in loop discovery's
/// per-timestep DFS sweep. Discovery on very large models can be infeasibly
/// slow (GH #647), so callers that want a bounded run pass `Some(duration)`;
/// the returned `ModelAnalysis::truncated` reports whether the budget elapsed
/// before discovery finished. `None` runs discovery to completion.
///
/// Returns `Err` when `model_name` does not match any model in the project,
/// unless `model_name` is `"main"` (which falls back to the first model for
/// single-model project convenience).
pub fn analyze_model(
    project: &datamodel::Project,
    db: &mut SimlinDb,
    source_project: SourceProject,
    model_name: &str,
    budget: Option<std::time::Duration>,
) -> Result<ModelAnalysis, String> {
    use salsa::Setter;

    let json_model = model_snapshot(project, model_name)
        .ok_or_else(|| format!("model '{model_name}' not found in project"))?;

    // Enable LTM discovery for this analysis; restore flags before returning
    // so the caller's db state stays clean.
    source_project.set_ltm_enabled(db).to(true);
    source_project.set_ltm_discovery_mode(db).to(true);

    let loop_result = run_ltm_pipeline(project, db, source_project, model_name, budget);

    source_project.set_ltm_enabled(db).to(false);
    source_project.set_ltm_discovery_mode(db).to(false);

    match loop_result {
        Some(result) => Ok(ModelAnalysis {
            model: json_model,
            time: result.time,
            loop_dominance: result.loop_dominance,
            partitions: result.partitions,
            dominant_loops_by_period: result.dominant_loops_by_period,
            truncated: result.truncated,
            agg_recovery_truncated: result.agg_recovery_truncated,
        }),
        None => Ok(ModelAnalysis {
            model: json_model,
            time: vec![],
            loop_dominance: vec![],
            partitions: vec![],
            dominant_loops_by_period: vec![],
            truncated: false,
            agg_recovery_truncated: false,
        }),
    }
}

/// The loop-bearing half of a successful `run_ltm_pipeline` run: the time
/// array, the discovered loop summaries, the dominant-period intervals, and
/// whether discovery was truncated by the time budget. Named (rather than a
/// bare tuple) so the signature stays readable and clippy's complex-type lint
/// is satisfied.
struct PipelineResult {
    time: Vec<f64>,
    loop_dominance: Vec<LoopSummary>,
    partitions: Vec<crate::ltm_finding::DiscoveredPartition>,
    dominant_loops_by_period: Vec<DominantPeriod>,
    truncated: bool,
    agg_recovery_truncated: bool,
}

/// Run the full LTM discovery pipeline.  Returns `None` on any failure.
///
/// Uses the caller-provided salsa `SimlinDb` and `SourceProject` for
/// both compilation/simulation and structural loop analysis (via
/// `model_causal_edges` / `causal_graph_from_edges`).
fn run_ltm_pipeline(
    project: &datamodel::Project,
    db: &mut SimlinDb,
    source_project: SourceProject,
    model_name: &str,
    budget: Option<std::time::Duration>,
) -> Option<PipelineResult> {
    let matched_model = project.models.iter().find(|m| {
        crate::canonicalize(&m.name).as_ref() == crate::canonicalize(model_name).as_ref()
    });
    let dm_model = matched_model.or_else(|| project.models.first())?;

    // Original name for datamodel lookups (get_model does exact matching);
    // canonical name for salsa map lookups (models(db) uses canonical keys).
    let original_name = &dm_model.name;
    let canonical_name = crate::canonicalize(original_name).into_owned();

    let uid_to_loop_name = build_uid_to_loop_name(project, original_name);

    // LTM flags are set by the caller (analyze_model) before this function
    // is called, and restored after it returns.

    let compiled_sim =
        crate::db::compile_project_incremental(db, source_project, &canonical_name).ok()?;
    let mut vm = crate::vm::Vm::new(compiled_sim).ok()?;
    vm.run_to_end().ok()?;
    let results = vm.into_results();

    // Build an element-level CausalGraph for loop discovery. This ensures
    // that arrayed models get element-specific loops (e.g., population[NYC]
    // -> births[NYC] -> population[NYC]) rather than variable-level loops.
    let source_model = source_project.models(db).get(&canonical_name).copied()?;
    let element_edges = crate::db::model_element_causal_edges(db, source_model, source_project);
    // Enrich the element-level graph with module sub-graphs + variable map so
    // the discovery-mode per-exit-port pathway recompute (GH #698) can fire on
    // this production path; the bare `causal_graph_from_element_edges`
    // constructor leaves both empty.
    let causal_graph = crate::db::causal_graph_from_element_edges_with_modules(
        db,
        source_model,
        source_project,
        element_edges,
    );

    let stocks: Vec<crate::common::Ident<crate::common::Canonical>> = element_edges
        .stocks
        .iter()
        .map(|s| crate::common::Ident::new(s))
        .collect();

    // Get LTM variable metadata and project dimensions for A2A link
    // score expansion. This allows parse_link_offsets to expand A2A
    // link scores into per-element edges.
    let ltm_vars = crate::db::model_ltm_variables(db, source_model, source_project);
    let dm_dims = crate::db::project_datamodel_dims(db, source_project);

    let discovery = crate::ltm_finding::discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm_vars.vars,
        dm_dims,
        budget,
    )
    .ok()?;
    let found_loops = discovery.loops;
    let partitions = discovery.partitions;
    let truncated = discovery.truncated;
    let agg_recovery_truncated = discovery.agg_recovery_truncated;

    let time = build_time_array(&results);

    let feedback_loops: Vec<FeedbackLoop> = found_loops.iter().map(to_feedback_loop).collect();

    let dominant_loops_by_period = metadata::calculate_dominant_periods(
        &feedback_loops,
        results.specs.start,
        results.specs.save_step,
    );

    let loop_dominance: Vec<LoopSummary> = found_loops
        .iter()
        .map(|fl| to_loop_summary(fl, &uid_to_loop_name, original_name, project))
        .collect();

    Some(PipelineResult {
        time,
        loop_dominance,
        partitions,
        dominant_loops_by_period,
        truncated,
        agg_recovery_truncated,
    })
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
    // metadata::LoopPolarity is a 3-way coarse enum; the mostly-* variants
    // collapse into their dominant cousin since the layout-side legend
    // does not visually distinguish them today.
    let polarity = match fl.loop_info.polarity {
        crate::ltm::LoopPolarity::Reinforcing | crate::ltm::LoopPolarity::MostlyReinforcing => {
            metadata::LoopPolarity::Reinforcing
        }
        crate::ltm::LoopPolarity::Balancing | crate::ltm::LoopPolarity::MostlyBalancing => {
            metadata::LoopPolarity::Balancing
        }
        crate::ltm::LoopPolarity::Undetermined => metadata::LoopPolarity::Undetermined,
    };

    let variables = loop_variables(fl);

    // Feed the SIGNED partition-relative loop score into dominant-period
    // selection so periods are share-based, not raw-magnitude-based: a loop in
    // a high-raw-magnitude partition no longer dominates the period labels just
    // because its absolute score is large.  `rel_scores` is already normalized
    // per partition into [-1, 1] (see `to_loop_summary`); NaN coerces to 0 so
    // `calculate_dominant_periods` sees a finite series.
    let importance_series: Vec<f64> = signed_relative_importance(fl);

    FeedbackLoop {
        name: fl.loop_info.id.clone(),
        polarity,
        variables,
        importance_series,
        dominant_period: None,
    }
}

/// The loop's SIGNED partition-relative importance series (in [-1, 1]) with
/// non-finite entries coerced to 0, ready for `LoopSummary.importance` and
/// `calculate_dominant_periods`.
///
/// `FoundLoop.rel_scores` is populated by `ltm_finding::rank_truncate_and_id`
/// from the cycle-partition denominators; it is empty only on the no-score-data
/// path (no timesteps), in which case this returns an empty series.  Raw loop
/// scores are NOT comparable across partitions (GH #543), so importance and
/// dominant-period selection use this relative series instead.
fn signed_relative_importance(fl: &crate::ltm_finding::FoundLoop) -> Vec<f64> {
    fl.rel_scores
        .iter()
        .map(|s| if s.is_finite() { *s } else { 0.0 })
        .collect()
}

/// Extract the ordered variable names from a `FoundLoop`.
fn loop_variables(fl: &crate::ltm_finding::FoundLoop) -> Vec<String> {
    // The bare node sequence around the cycle (each link's `from`), WITHOUT a
    // trailing repeat of the first node. This matches the structural-loop
    // convention (`db::analysis` `model_detected_loops`) so both kinds of loop
    // populate the same `Loop` type consistently: consumers that render the
    // cycle (e.g. pysimlin's `Loop.__str__`) close it themselves by appending
    // the first variable, and a stored repeat would double that closing node.
    fl.loop_info
        .links
        .iter()
        .map(|l| l.from.to_string())
        .collect()
}

/// Convert a `FoundLoop` to a `LoopSummary`, resolving a human-readable
/// name from persisted `loop_metadata` when the UID sets match.
fn to_loop_summary(
    fl: &crate::ltm_finding::FoundLoop,
    uid_to_loop_name: &[(Vec<i32>, String)],
    model_name: &str,
    project: &datamodel::Project,
) -> LoopSummary {
    // The five-string surface mirrors `LoopPolarity::abbreviation` (R/B/Rux/
    // Bux/U) so consumers reading the JSON polarity field see the same
    // vocabulary the LTM literature uses.  AI-agent docs in
    // simlin-mcp/src/instructions.md and skills/loop-dominance.md are kept
    // in sync; if you add another variant, update those too.
    let polarity = match fl.loop_info.polarity {
        crate::ltm::LoopPolarity::Reinforcing => "reinforcing",
        crate::ltm::LoopPolarity::Balancing => "balancing",
        crate::ltm::LoopPolarity::MostlyReinforcing => "mostly_reinforcing",
        crate::ltm::LoopPolarity::MostlyBalancing => "mostly_balancing",
        crate::ltm::LoopPolarity::Undetermined => "undetermined",
    }
    .to_string();

    let variables = loop_variables(fl);

    // `importance` is the SIGNED partition-relative loop score in [-1, 1], not
    // the raw |loop score|.  Raw scores are incomparable across cycle
    // partitions (one partition's loops can score in the tens of thousands
    // while another's score in the tens), so a raw |score| is meaningless as an
    // "importance" number and inconsistent with both the engine's own
    // mean-relative ranking (GH #543) and the pinned-loop path's [-1, 1] scores.
    let importance: Vec<f64> = signed_relative_importance(fl);

    let name = resolve_loop_name(fl, uid_to_loop_name, model_name, project);

    LoopSummary {
        loop_id: fl.loop_info.id.clone(),
        name,
        polarity,
        variables,
        importance,
        partition: fl.partition,
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
    project: &datamodel::Project,
) -> Option<String> {
    if uid_to_loop_name.is_empty() {
        return None;
    }

    // Collect UIDs for variables in the loop from the datamodel,
    // restricted to the model being analyzed.
    let loop_var_idents: std::collections::HashSet<String> = fl
        .loop_info
        .links
        .iter()
        .map(|l| l.from.to_string())
        .collect();

    let mut loop_uids: Vec<i32> = project
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

    fn synced_db(project: &datamodel::Project) -> (SimlinDb, SourceProject) {
        let db = SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, project);
        let sp = sync.project;
        (db, sp)
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
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        assert!(
            !analysis.loop_dominance.is_empty(),
            "expected non-empty loop_dominance, got none"
        );
        assert!(
            !analysis.dominant_loops_by_period.is_empty(),
            "expected non-empty dominant_loops_by_period, got none"
        );
    }

    // ---- budget: a generous budget completes; a near-zero budget truncates ----

    #[test]
    fn generous_budget_completes_without_truncation() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let (mut db, sp) = synced_db(&project);
        // A budget far larger than this tiny model needs must complete fully.
        let analysis = analyze_model(
            &project,
            &mut db,
            sp,
            "main",
            Some(std::time::Duration::from_secs(60)),
        )
        .expect("analyze_model failed");

        assert!(
            !analysis.truncated,
            "a 60s budget must not truncate discovery on a tiny model"
        );
        assert!(
            !analysis.loop_dominance.is_empty(),
            "a completed analysis must still return loops"
        );
    }

    #[test]
    fn zero_budget_truncates_without_hanging() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let (mut db, sp) = synced_db(&project);
        // A zero budget is already elapsed before the first timestep, so the
        // per-step sweep stops immediately and reports truncation. The loop
        // fields are whatever (possibly nothing) was found first; the contract
        // we assert is the truncation flag and that the call returns promptly.
        let analysis = analyze_model(
            &project,
            &mut db,
            sp,
            "main",
            Some(std::time::Duration::ZERO),
        )
        .expect("analyze_model failed");

        assert!(
            analysis.truncated,
            "a zero budget must report truncated discovery"
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
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

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

    // ---- importance is the signed partition-relative loop score ----

    /// `analyze_model`'s `LoopSummary.importance` is the *signed
    /// partition-relative* loop score in [-1, 1], NOT the raw |loop score|.
    /// On the two-loop logistic-growth model (one reinforcing growth loop and
    /// one balancing carrying-capacity loop sharing a cycle partition):
    ///   * every importance value lies in [-1, 1],
    ///   * where both loops are active the |importance| values sum to ~1.0
    ///     (they normalize against their shared partition total),
    ///   * the balancing loop carries a negative importance somewhere (sign
    ///     preserved), the reinforcing one a positive importance, and
    ///   * loops arrive sorted by mean |relative importance| descending.
    #[test]
    fn importance_is_signed_partition_relative_score() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        assert_eq!(
            analysis.loop_dominance.len(),
            2,
            "logistic growth has exactly two feedback loops"
        );

        // Every importance value is a relative score in [-1, 1].
        for summary in &analysis.loop_dominance {
            for &v in &summary.importance {
                assert!(
                    (-1.0..=1.0).contains(&v),
                    "importance {v} for loop {} must lie in [-1, 1]",
                    summary.loop_id
                );
            }
        }

        // The two loops share one cycle partition, so at any step where both
        // are active their |importance| values sum to ~1.0.  Find such a step.
        let n = analysis.time.len();
        let a = &analysis.loop_dominance[0].importance;
        let b = &analysis.loop_dominance[1].importance;
        let both_active_sum_to_one = (0..n).any(|t| {
            let (x, y) = (a[t], b[t]);
            x != 0.0 && y != 0.0 && ((x.abs() + y.abs()) - 1.0).abs() < 1e-6
        });
        assert!(
            both_active_sum_to_one,
            "where both loops are active their |importance| must sum to ~1.0"
        );

        // The balancing (carrying-capacity) loop's importance is negative
        // somewhere; the reinforcing one's is positive somewhere.  Sign is
        // preserved through the relative-score normalization.
        let has_negative = analysis
            .loop_dominance
            .iter()
            .any(|s| s.importance.iter().any(|&v| v < 0.0));
        let has_positive = analysis
            .loop_dominance
            .iter()
            .any(|s| s.importance.iter().any(|&v| v > 0.0));
        assert!(
            has_negative,
            "a balancing loop must carry a negative signed importance"
        );
        assert!(
            has_positive,
            "a reinforcing loop must carry a positive signed importance"
        );

        // Loops arrive sorted by mean |relative importance| descending.
        let mean_abs = |s: &LoopSummary| -> f64 {
            let active: Vec<f64> = s.importance.iter().copied().filter(|v| *v != 0.0).collect();
            if active.is_empty() {
                0.0
            } else {
                active.iter().map(|v| v.abs()).sum::<f64>() / active.len() as f64
            }
        };
        let means: Vec<f64> = analysis.loop_dominance.iter().map(mean_abs).collect();
        for w in means.windows(2) {
            assert!(
                w[0] >= w[1] - 1e-9,
                "loops must be sorted by mean |relative importance| descending: {means:?}"
            );
        }
    }

    // ---- partition metadata on the discovery surface ----

    /// `analyze_model` exposes each loop's result-scoped cycle partition and
    /// the partition list itself.  Logistic growth has one stock
    /// (`population`), so both loops share partition 0 and the partition list
    /// holds exactly that one entry with `loop_count == 2`.
    #[test]
    fn partition_metadata_is_populated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        assert_eq!(analysis.loop_dominance.len(), 2);
        assert_eq!(
            analysis.partitions.len(),
            1,
            "one stock means one cycle partition"
        );
        assert_eq!(analysis.partitions[0].loop_count, 2);
        assert!(
            analysis.partitions[0]
                .stocks
                .iter()
                .any(|s| s.contains("population")),
            "the partition's stocks must name the model's stock; got {:?}",
            analysis.partitions[0].stocks
        );
        for summary in &analysis.loop_dominance {
            assert_eq!(
                summary.partition,
                Some(0),
                "both loops share the single (dense index 0) partition"
            );
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
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

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
        let (mut db, sp) = synced_db(&project);
        let analysis = analyze_model(&project, &mut db, sp, "main", None)
            .expect("analyze_model should not return Err");

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

    // ---- unknown model name returns Err ----

    #[test]
    fn unknown_model_name_returns_err() {
        let project = crate::test_common::TestProject::new("main")
            .stock("population", "1000", &["births"], &["deaths"], None)
            .flow("births", "population * 0.03", None)
            .flow("deaths", "population * 0.01", None)
            .build_datamodel();

        let (mut db, sp) = synced_db(&project);
        let result = analyze_model(&project, &mut db, sp, "nonexistent", None);
        assert!(
            result.is_err(),
            "analyze_model with an unknown model name must return Err, got Ok"
        );
        let err = result.err().unwrap();
        assert!(
            err.contains("nonexistent"),
            "error message should mention the missing model name, got: {err}"
        );
    }

    // ---- "main" alias falls back to first model ----

    #[test]
    fn main_alias_falls_back_to_first_model() {
        // Build a project whose single model is named "Main" (not "main").
        // The "main" alias must still find it via get_model's special-case logic.
        let project = crate::test_common::TestProject::new("Main")
            .stock("population", "1000", &[], &[], None)
            .build_datamodel();

        let (mut db, sp) = synced_db(&project);
        let result = analyze_model(&project, &mut db, sp, "main", None);
        assert!(
            result.is_ok(),
            "analyze_model with 'main' alias must succeed even when the model is named 'Main'"
        );
    }

    // ---- non-canonical model name must not break causal graph lookup ----

    #[test]
    fn non_canonical_model_name_finds_causal_edges() {
        // Model name "Main" (uppercase) must still produce loop results,
        // because models(db) is keyed by canonical names.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let mut project = load_project(path);
        // Rename the model to a non-canonical form
        project.models[0].name = "Main".to_string();

        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "Main", None).expect("analyze_model failed");

        assert!(
            !analysis.loop_dominance.is_empty(),
            "non-canonical model name must still produce loop results"
        );
    }

    // ---- LTM flags must be reset after analyze_model ----

    #[test]
    fn ltm_flags_reset_after_analyze() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let project = load_project(path);
        let (mut db, sp) = synced_db(&project);
        let _analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        // After analyze_model returns, LTM flags must be restored to false
        // so subsequent compilations don't unexpectedly run in LTM discovery mode.
        assert!(
            !sp.ltm_enabled(&db),
            "ltm_enabled must be false after analyze_model returns"
        );
        assert!(
            !sp.ltm_discovery_mode(&db),
            "ltm_discovery_mode must be false after analyze_model returns"
        );
    }

    #[test]
    fn ltm_flags_reset_after_failed_analysis() {
        let project = broken_project();
        let (mut db, sp) = synced_db(&project);
        let _analysis = analyze_model(&project, &mut db, sp, "main", None)
            .expect("analyze_model should not return Err");

        assert!(
            !sp.ltm_enabled(&db),
            "ltm_enabled must be false after failed analyze_model"
        );
        assert!(
            !sp.ltm_discovery_mode(&db),
            "ltm_discovery_mode must be false after failed analyze_model"
        );
    }

    /// A stockless passthrough sub-model exposing two outputs of opposing
    /// sign from one input: `pos = input_val * 0.02`, `neg = 0 - input_val`.
    /// `input_val` is a real input port; both `pos` and `neg` are read by the
    /// parent so both are output ports.
    fn pos_neg_passthrough_model() -> datamodel::Model {
        use crate::testutils::{x_aux, x_model};
        let input = datamodel::Variable::Aux(datamodel::Aux {
            ident: "input_val".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: true,
                ..datamodel::Compat::default()
            },
        });
        x_model(
            "passthrough",
            vec![
                input,
                x_aux("pos", "input_val * 0.02", None),
                x_aux("neg", "0 - input_val", None),
            ],
        )
    }

    /// A module instance with a distinct sub-model name (the `x_module`
    /// helper forces `model_name == ident`, which can't instantiate a
    /// differently-named sub-model).
    fn module_instance(
        ident: &str,
        model_name: &str,
        refs: &[(&str, &str)],
    ) -> datamodel::Variable {
        datamodel::Variable::Module(datamodel::Module {
            ident: ident.to_string(),
            model_name: model_name.to_string(),
            documentation: String::new(),
            units: None,
            references: refs
                .iter()
                .map(|(src, dst)| datamodel::ModuleReference {
                    src: src.to_string(),
                    dst: dst.to_string(),
                })
                .collect(),
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        })
    }

    /// GH #698 (production path): `analyze_model` runs the discovery pipeline
    /// through `causal_graph_from_element_edges`, not the `discover_loops`
    /// convenience wrapper. A feedback loop traversing a multi-output module
    /// must report the SAME polarity discovery's sibling exhaustive score
    /// reports (+1, reinforcing -- pinned by
    /// `multi_output_passthrough_loop_raw_score_is_one` in
    /// tests/simulate_ltm.rs). Before the production-path fix the
    /// element-level graph carried no module sub-graph, so the per-exit-port
    /// recompute could not fire and discovery reported balancing (the
    /// composite tie-break selected the wrong-signed `neg` port).
    #[test]
    fn analyze_model_multi_output_loop_polarity_reinforcing() {
        use crate::testutils::{x_aux, x_flow, x_model, x_stock};

        let project = datamodel::Project {
            name: "multi_output_loop".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 8.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![
                x_model(
                    "main",
                    vec![
                        x_stock("s", "100", &["growth"], &[], None),
                        module_instance("m", "passthrough", &[("s", "m.input_val")]),
                        x_flow("growth", "m.pos * 0.1", None),
                        // Reads the OTHER output so `neg` is also an output port.
                        x_aux("watcher", "m.neg", None),
                    ],
                ),
                pos_neg_passthrough_model(),
            ],
            source: None,
            ai_information: None,
        };

        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        let loop_through_m = analysis
            .loop_dominance
            .iter()
            .find(|l| l.variables.iter().any(|v| v == "m"))
            .expect("a discovered loop must traverse module m");

        // The loop reads m.pos (positive gain): reinforcing, matching the
        // exhaustive raw loop score of +1.
        assert_eq!(
            loop_through_m.polarity, "reinforcing",
            "discovery must report reinforcing for a loop reading m.pos; got {} (vars {:?}). \
             The composite tie-break selected the wrong-signed neg port. GH #698.",
            loop_through_m.polarity, loop_through_m.variables
        );
        let settled = loop_through_m
            .importance
            .iter()
            .rev()
            .find(|s| s.is_finite() && **s != 0.0)
            .copied()
            .expect("loop must have a finite non-zero importance");
        assert!(
            settled > 0.0,
            "discovery settled importance is {settled}; expected positive (reinforcing). GH #698."
        );
    }

    /// GH #698, module->module exit-port arm: the loop's exit hop is
    /// `mod_a -> mod_b` where `mod_b` is itself a module reading `mod_a·pos`
    /// on its input port. `recompute_module_input_edge_series` must read the
    /// exit port off `mod_b`'s `ModuleInput.src` (the
    /// `Variable::Module { inputs, .. }` arm), not off an aux reader. `mod_b`
    /// is a single-output identity passthrough (`out = input`), so the loop
    /// polarity follows `mod_a`'s `pos` port: reinforcing.
    fn module_to_module_multi_output_project() -> datamodel::Project {
        use crate::testutils::{x_aux, x_flow, x_model, x_stock};

        // Single-output identity passthrough: `out = input` (+1 gain).
        let identity_model = {
            let input = datamodel::Variable::Aux(datamodel::Aux {
                ident: "input".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: true,
                    ..datamodel::Compat::default()
                },
            });
            x_model("passthrough_b", vec![input, x_aux("out", "input", None)])
        };

        // mod_a's multi-output sub-model: pos = input * 0.02, neg = 0 - input.
        let pos_neg = {
            let input = datamodel::Variable::Aux(datamodel::Aux {
                ident: "input".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: true,
                    ..datamodel::Compat::default()
                },
            });
            x_model(
                "passthrough_a",
                vec![
                    input,
                    x_aux("pos", "input * 0.02", None),
                    x_aux("neg", "0 - input", None),
                ],
            )
        };

        datamodel::Project {
            name: "module_to_module_loop".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 8.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![
                x_model(
                    "main",
                    vec![
                        x_stock("level", "100", &["inflow"], &[], None),
                        // mod_a: multi-output passthrough (pos/neg) fed by level.
                        module_instance("mod_a", "passthrough_a", &[("level", "mod_a.input")]),
                        // mod_b: identity module fed by mod_a.pos -- the loop's
                        // exit hop is the module->module edge mod_a -> mod_b.
                        module_instance("mod_b", "passthrough_b", &[("mod_a.pos", "mod_b.input")]),
                        x_flow("inflow", "mod_b.out * 0.1", None),
                        // Force neg into mod_a's output-port set.
                        x_aux("watcher", "mod_a.neg", None),
                    ],
                ),
                pos_neg,
                identity_model,
            ],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn analyze_model_module_to_module_exit_port_polarity_reinforcing() {
        let project = module_to_module_multi_output_project();
        let (mut db, sp) = synced_db(&project);
        let analysis =
            analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model failed");

        let loop_through_modules = analysis
            .loop_dominance
            .iter()
            .find(|l| {
                l.variables.iter().any(|v| v == "mod_a") && l.variables.iter().any(|v| v == "mod_b")
            })
            .unwrap_or_else(|| {
                panic!(
                    "a discovered loop must traverse both modules; loops: {:?}",
                    analysis
                        .loop_dominance
                        .iter()
                        .map(|l| &l.variables)
                        .collect::<Vec<_>>()
                )
            });

        assert_eq!(
            loop_through_modules.polarity, "reinforcing",
            "module->module exit port read off mod_a must select the pos port; got {} (vars {:?}). \
             GH #698.",
            loop_through_modules.polarity, loop_through_modules.variables
        );
    }
}
