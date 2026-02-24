// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP-facing output types shared between ReadModel and EditModel tools.

use serde::Serialize;

/// Per-loop dominance summary included in tool output.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopDominanceSummary {
    pub loop_id: String,
    pub name: Option<String>,
    pub polarity: String,
    pub variables: Vec<String>,
    pub importance: Vec<f64>,
}

impl From<simlin_engine::analysis::LoopSummary> for LoopDominanceSummary {
    fn from(ls: simlin_engine::analysis::LoopSummary) -> Self {
        Self {
            loop_id: ls.loop_id,
            name: ls.name,
            polarity: ls.polarity,
            variables: ls.variables,
            importance: ls.importance,
        }
    }
}

/// A time interval during which specific loops dominate model behavior.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DominantPeriodOutput {
    pub dominant_loops: Vec<String>,
    pub start_time: f64,
    pub end_time: f64,
}

impl From<simlin_engine::layout::metadata::DominantPeriod> for DominantPeriodOutput {
    fn from(dp: simlin_engine::layout::metadata::DominantPeriod) -> Self {
        Self {
            dominant_loops: dp.dominant_loops,
            start_time: dp.start,
            end_time: dp.end,
        }
    }
}
