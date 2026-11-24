// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Replay a model's construction the way an agent or a notebook user builds
//! one: a few edits, each adding a group of variables, with the diagram synced
//! after every edit by the production path -- `generate_best_layout` while the
//! view is empty, `incremental_layout` after that (the rule MCP `edit_model`
//! and pysimlin's patch sync follow). The final diagram is what such a user
//! ends up looking at, and it can be very different from a fresh layout of the
//! finished model: incremental layout preserves everything already placed.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::Instant;

use simlin_engine::datamodel::{self, StockFlow, Variable};
use simlin_engine::layout::{compute_layout_metadata, generate_best_layout, incremental_layout};
use simlin_engine::{ModelOperation, ModelPatch, ProjectPatch, apply_patch, canonicalize};

use crate::corpus::MAIN_MODEL;

/// The replayed build: its final view, how many edits it took, and the total
/// wall-clock milliseconds of every diagram sync along the way.
pub struct Replay {
    pub view: StockFlow,
    pub steps: usize,
    pub elapsed_ms: f64,
}

/// The model's variables grouped into build units in the order a person builds
/// them: each stock-flow chain as one unit (a stock arrives together with its
/// flows, so no flow references an absent stock), largest first; then every
/// other variable singly, nearest the backbone first (breadth-first over the
/// dependency graph from the chains), with unconnected variables last by name.
fn build_units<'a>(
    project: &datamodel::Project,
    model: &'a datamodel::Model,
) -> Result<Vec<Vec<&'a Variable>>, String> {
    let metadata = compute_layout_metadata(project, MAIN_MODEL, None)
        .ok_or_else(|| "no layout metadata".to_string())?;
    let by_ident: HashMap<String, &Variable> = model
        .variables
        .iter()
        .map(|v| (canonicalize(v.get_ident()).into_owned(), v))
        .collect();

    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut units: Vec<Vec<&Variable>> = Vec::new();
    let mut chains: Vec<&simlin_engine::layout::metadata::StockFlowChain> =
        metadata.chains.iter().collect();
    chains.sort_by(|a, b| {
        b.all_vars
            .len()
            .cmp(&a.all_vars.len())
            .then_with(|| a.all_vars.cmp(&b.all_vars))
    });
    for chain in chains {
        let unit: Vec<&Variable> = chain
            .all_vars
            .iter()
            .filter(|ident| placed.insert((*ident).clone()))
            .filter_map(|ident| by_ident.get(ident).copied())
            .collect();
        if !unit.is_empty() {
            units.push(unit);
        }
    }

    // Breadth-first from the backbone over dependencies in both directions.
    let neighbors = |ident: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for graph in [&metadata.dep_graph, &metadata.reverse_dep_graph] {
            if let Some(set) = graph.get(ident) {
                out.extend(set.iter().cloned());
            }
        }
        out
    };
    let mut queue: VecDeque<String> = placed.iter().cloned().collect();
    while let Some(ident) = queue.pop_front() {
        for next in neighbors(&ident) {
            if by_ident.contains_key(&next) && placed.insert(next.clone()) {
                units.push(vec![by_ident[&next]]);
                queue.push_back(next);
            }
        }
    }
    let mut rest: Vec<&String> = by_ident.keys().filter(|k| !placed.contains(*k)).collect();
    rest.sort();
    for ident in rest {
        units.push(vec![by_ident[ident]]);
    }
    Ok(units)
}

/// Split ordered units into at most `steps` contiguous edits of roughly equal
/// variable counts, never splitting a unit.
fn batch_units(units: Vec<Vec<&Variable>>, steps: usize) -> Vec<Vec<&Variable>> {
    let total: usize = units.iter().map(Vec::len).sum();
    let target = total.div_ceil(steps.max(1)).max(1);
    let mut batches: Vec<Vec<&Variable>> = Vec::new();
    let mut current: Vec<&Variable> = Vec::new();
    for unit in units {
        if !current.is_empty() && current.len() + unit.len() > target && batches.len() + 1 < steps {
            batches.push(std::mem::take(&mut current));
        }
        current.extend(unit);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn upsert(var: &Variable) -> ModelOperation {
    match var {
        Variable::Stock(s) => ModelOperation::UpsertStock(s.clone()),
        Variable::Flow(f) => ModelOperation::UpsertFlow(f.clone()),
        Variable::Aux(a) => ModelOperation::UpsertAux(a.clone()),
        Variable::Module(m) => ModelOperation::UpsertModule(m.clone()),
    }
}

/// Build `project`'s main model from empty in `steps` edits, syncing the
/// diagram after each, and return the final diagram.
pub fn replay(key: &str, project: &datamodel::Project, steps: usize) -> Option<Replay> {
    let run = || -> Result<Replay, String> {
        let model = project
            .get_model(MAIN_MODEL)
            .ok_or_else(|| "no main model".to_string())?;
        let model_name = model.name.clone();
        let batches = batch_units(build_units(project, model)?, steps);

        let mut building = project.clone();
        {
            let empty = building
                .get_model_mut(MAIN_MODEL)
                .ok_or_else(|| "no main model".to_string())?;
            empty.variables.clear();
            empty.views.clear();
            empty.loop_metadata.clear();
        }

        let mut view: Option<StockFlow> = None;
        let mut elapsed_ms = 0.0;
        for batch in &batches {
            let model_patch = ModelPatch {
                name: model_name.clone(),
                ops: batch.iter().map(|v| upsert(v)).collect(),
            };
            apply_patch(
                &mut building,
                ProjectPatch {
                    project_ops: vec![],
                    models: vec![model_patch.clone()],
                },
            )
            .map_err(|e| format!("patch failed: {e:?}"))?;
            let start = Instant::now();
            let synced = match &view {
                Some(old) if !old.elements.is_empty() => {
                    incremental_layout(old, &building, MAIN_MODEL, &model_patch, None)
                }
                _ => generate_best_layout(&building, MAIN_MODEL, None),
            }?;
            elapsed_ms += start.elapsed().as_secs_f64() * 1000.0;
            if let Some(m) = building.get_model_mut(MAIN_MODEL) {
                m.views = vec![datamodel::View::StockFlow(synced.clone())];
            }
            view = Some(synced);
        }
        let view = view.ok_or_else(|| "model has no variables".to_string())?;
        Ok(Replay {
            view,
            steps: batches.len(),
            elapsed_ms,
        })
    };
    match run() {
        Ok(replay) => Some(replay),
        Err(err) => {
            eprintln!("WARN: {key} incremental replay failed: {err}");
            None
        }
    }
}
