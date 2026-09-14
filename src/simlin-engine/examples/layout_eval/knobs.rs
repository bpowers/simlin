// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Environment knobs, parsed once at startup.

use std::env;

use crate::corpus::Tier;

/// Default number of seeds to sample per model when `LAYOUT_EVAL_SEEDS` is unset.
const DEFAULT_SEEDS: u64 = 25;

/// Default number of edits in the incremental-build replay: a handful, like an
/// agent's session of `edit_model` calls.
const DEFAULT_REPLAY_STEPS: usize = 4;

pub struct Knobs {
    /// `LAYOUT_EVAL_MODELS`: corpus keys to run (`None` = all).
    pub models: Option<Vec<String>>,
    /// `LAYOUT_EVAL_TIERS`: size classes to run (`None` = all).
    pub tiers: Option<Vec<Tier>>,
    /// `LAYOUT_EVAL_EXTRA`: ad-hoc `key=path` models appended to the corpus.
    pub extra: Vec<(String, String)>,
    /// `LAYOUT_EVAL_SEEDS`: seeds sampled per model.
    pub seeds: u64,
    /// `LAYOUT_EVAL_OUT`: output directory.
    pub out: String,
    /// `LAYOUT_EVAL_WRITE_BASELINE`: re-seed the committed baseline instead of
    /// diffing against it.
    pub write_baseline: bool,
    /// `LAYOUT_EVAL_COMPARE`: a previous run's output dir to diff against.
    pub compare_dir: Option<String>,
    /// `LAYOUT_EVAL_DECLUTTER=0` disables the declutter pass in the seed sweep.
    pub declutter: bool,
    /// `LAYOUT_EVAL_REPLAY_STEPS`: edits in the incremental-build replay; 0
    /// skips it.
    pub replay_steps: usize,
    /// `LAYOUT_EVAL_EDITS=0` skips the edit scenarios.
    pub edits: bool,
}

fn list(name: &str) -> Option<Vec<String>> {
    let raw = env::var(name).ok()?;
    let items: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if items.is_empty() { None } else { Some(items) }
}

fn flag(name: &str) -> bool {
    matches!(
        env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true"
    )
}

impl Knobs {
    pub fn from_env() -> Knobs {
        let tiers = list("LAYOUT_EVAL_TIERS").map(|names| {
            names
                .iter()
                .filter_map(|n| {
                    let tier = Tier::parse(n);
                    if tier.is_none() {
                        eprintln!("WARN: unknown tier {n:?} (small|medium|large); ignoring");
                    }
                    tier
                })
                .collect()
        });
        let extra = list("LAYOUT_EVAL_EXTRA")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| match item.split_once('=') {
                Some((key, path)) if !key.is_empty() && !path.is_empty() => {
                    Some((key.to_string(), path.to_string()))
                }
                _ => {
                    eprintln!("WARN: LAYOUT_EVAL_EXTRA entry {item:?} is not key=path; ignoring");
                    None
                }
            })
            .collect();
        Knobs {
            models: list("LAYOUT_EVAL_MODELS"),
            tiers,
            extra,
            seeds: env::var("LAYOUT_EVAL_SEEDS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_SEEDS),
            out: env::var("LAYOUT_EVAL_OUT").unwrap_or_else(|_| {
                format!("{}/../../target/layout-eval", env!("CARGO_MANIFEST_DIR"))
            }),
            write_baseline: flag("LAYOUT_EVAL_WRITE_BASELINE"),
            compare_dir: env::var("LAYOUT_EVAL_COMPARE")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            declutter: !matches!(
                env::var("LAYOUT_EVAL_DECLUTTER").unwrap_or_default().trim(),
                "0" | "false"
            ),
            replay_steps: env::var("LAYOUT_EVAL_REPLAY_STEPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_REPLAY_STEPS),
            edits: !matches!(
                env::var("LAYOUT_EVAL_EDITS").unwrap_or_default().trim(),
                "0" | "false"
            ),
        }
    }
}
