// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP-facing output types shared between ReadModel and EditModel tools.

use serde::Serialize;

/// Rounds a float to 3 significant figures via scientific-notation round-trip.
/// Mirrors Go's `strconv.FormatFloat(v, 'g', 3, 64)` behavior.
fn round_sig_figs_3(v: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let s = format!("{:.2e}", v);
    s.parse::<f64>().unwrap_or(v)
}

/// Serializes an importance array with values rounded to 3 significant figures,
/// reducing token count in MCP tool output.
fn serialize_importance<S: serde::Serializer>(
    values: &[f64],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(values.len()))?;
    for &v in values {
        seq.serialize_element(&round_sig_figs_3(v))?;
    }
    seq.end()
}

/// Per-loop dominance summary included in tool output.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopDominanceSummary {
    pub loop_id: String,
    pub name: Option<String>,
    pub polarity: String,
    pub variables: Vec<String>,
    #[serde(serialize_with = "serialize_importance")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_sig_figs_3_basic() {
        assert_eq!(round_sig_figs_3(2.449215777949112), 2.45);
    }

    #[test]
    fn round_sig_figs_3_zero() {
        assert_eq!(round_sig_figs_3(0.0), 0.0);
    }

    #[test]
    fn round_sig_figs_3_very_small() {
        assert_eq!(round_sig_figs_3(0.000004781283), 4.78e-6);
    }

    #[test]
    fn round_sig_figs_3_large() {
        assert_eq!(round_sig_figs_3(25.189), 25.2);
    }

    #[test]
    fn round_sig_figs_3_negative() {
        assert_eq!(round_sig_figs_3(-3.456), -3.46);
    }

    #[test]
    fn importance_serializes_rounded() {
        let summary = LoopDominanceSummary {
            loop_id: "L1".into(),
            name: None,
            polarity: "positive".into(),
            variables: vec![],
            importance: vec![2.449, 0.0, 0.000004781, 25.189],
        };
        let json = serde_json::to_value(&summary).unwrap();
        let arr = json["importance"].as_array().unwrap();
        assert_eq!(arr[0].as_f64().unwrap(), 2.45);
        assert_eq!(arr[1].as_f64().unwrap(), 0.0);
        assert_eq!(arr[2].as_f64().unwrap(), 4.78e-6);
        assert_eq!(arr[3].as_f64().unwrap(), 25.2);
    }

    #[test]
    fn importance_exact_values_unchanged() {
        let summary = LoopDominanceSummary {
            loop_id: "L2".into(),
            name: None,
            polarity: "negative".into(),
            variables: vec![],
            importance: vec![1.0, 100.0, 0.5],
        };
        let json = serde_json::to_value(&summary).unwrap();
        let arr = json["importance"].as_array().unwrap();
        assert_eq!(arr[0].as_f64().unwrap(), 1.0);
        assert_eq!(arr[1].as_f64().unwrap(), 100.0);
        assert_eq!(arr[2].as_f64().unwrap(), 0.5);
    }
}
