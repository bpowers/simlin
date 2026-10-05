// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What loop analysis says of a project, in a form two spellings of one
//! model can be compared by: every loop score series and the LTM
//! diagnostics.

use crate::datamodel::Project;
use crate::db::{
    SimlinDb, collect_all_diagnostics, compile_project_incremental, sync_from_datamodel_incremental,
};

const LOOP_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}loop_score\u{205A}";

/// The steps a loop score needs before it is defined (a link score reads
/// the step before, and a stock's flow the one before that).
const STARTUP_STEPS: usize = 3;

/// One project's loop analysis.
#[derive(Debug, PartialEq)]
pub(crate) struct LoopScores {
    /// Every slot of every loop score variable that is not zero at every
    /// step, as its values' bits after the startup steps (a zero of either
    /// sign as 0), sorted: two spellings of one model compare equal however
    /// their loops are numbered.
    pub(crate) loops: Vec<Vec<u64>>,
    /// The LTM diagnostics, sorted.
    pub(crate) diagnostics: Vec<String>,
    /// The loops the model's analysis reports (`analysis::analyze_model`,
    /// discovery for a model too large to enumerate): each one's importance
    /// series as bits, sorted, with how many there are.
    pub(crate) discovered: Vec<Vec<u64>>,
}

/// `project`'s loop analysis, or why it has none.
pub(crate) fn loop_scores(project: &Project) -> Result<LoopScores, String> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let main = project
        .models
        .iter()
        .find(|m| m.macro_spec.is_none())
        .map(|m| m.name.clone())
        .ok_or("no model")?;
    let mut diagnostics: Vec<String> =
        collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::On)
            .iter()
            .map(|d| format!("{:?}", d.error))
            .filter(|m| m.contains("LTM"))
            .collect();
    diagnostics.sort();
    let compiled = compile_project_incremental(&db, sync.project, &main, crate::db::LtmOverlay::On)
        .map_err(|e| format!("compile: {e:?}"))?;
    let mut vm = crate::Vm::new(compiled).map_err(|e| format!("{e:?}"))?;
    vm.run_to_end().map_err(|e| format!("{e:?}"))?;
    let results = vm.into_results();
    let mut offsets: Vec<usize> = results.offsets.values().copied().collect();
    offsets.sort_unstable();
    offsets.dedup();
    let end = |off: usize| {
        offsets
            .iter()
            .find(|o| **o > off)
            .copied()
            .unwrap_or(results.step_size)
    };
    let mut loops = Vec::new();
    for (name, &off) in results.offsets.iter() {
        if !name.as_str().starts_with(LOOP_SCORE_PREFIX) {
            continue;
        }
        for slot in off..end(off) {
            let series: Vec<u64> = results
                .iter()
                .skip(STARTUP_STEPS)
                .map(|row| row[slot])
                .map(|v| if v == 0.0 { 0 } else { v.to_bits() })
                .collect();
            if series.iter().any(|v| *v != 0) {
                loops.push(series);
            }
        }
    }
    loops.sort();
    let analysis = crate::analysis::analyze_model(project, &mut db, sync.project, &main, None)?;
    let mut discovered: Vec<Vec<u64>> = analysis
        .loop_dominance
        .iter()
        .map(|summary| {
            summary
                .importance
                .iter()
                .map(|v| if *v == 0.0 { 0 } else { v.to_bits() })
                .collect()
        })
        .collect();
    discovered.sort();
    Ok(LoopScores {
        loops,
        diagnostics,
        discovered,
    })
}
