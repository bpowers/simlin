// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Layout-quality evaluation sweep (on-demand; NOT part of `cargo test`).
//!
//! Lays out a curated corpus of models across many seeds, scores each layout
//! with the layout-quality metric, renders best/median/worst (and any
//! hand-authored reference) to PNG, and writes a metrics table (JSON), an HTML
//! contact-sheet, and a baseline diff -- all under a gitignored `target/` dir.
//!
//! This is a thin imperative shell over the metric core
//! (`layout::metrics::compute_layout_metrics`) and the statistics core
//! (`layout::eval_stats`). It loads each model via the public `open_xmile` /
//! `open_vensim` loaders (like `examples/backend_bench.rs`), runs
//! `generate_layout_with_config` per seed, scores, summarizes, renders, and
//! emits artifacts.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
//!   LAYOUT_EVAL_MODELS=teacup,sir cargo run ... --example layout_eval
//!
//! Env knobs:
//!   LAYOUT_EVAL_MODELS  comma list of corpus keys to run (default: all)
//!   LAYOUT_EVAL_SEEDS   number of seeds M to sample (default: 25)
//!   LAYOUT_EVAL_OUT     output directory (default: repo-root target/layout-eval)
//!
//! Requires `--features png_render,file_io`: `png_render` for `render_png`, and
//! `file_io` so Vensim corpus models that reference external data can load.

use std::collections::BTreeSet;
use std::env;
use std::io::BufReader;

use simlin_engine::layout::LAYOUT_SEEDS;
use simlin_engine::{datamodel, open_vensim, open_xmile};

/// The model name the layout pipeline and renderer operate on. `Project::get_model`
/// maps "main" to the single/main model (matching `tests/layout.rs`).
const MAIN_MODEL: &str = "main";

/// Default number of seeds to sample per model when `LAYOUT_EVAL_SEEDS` is unset.
const DEFAULT_SEEDS: u64 = 25;

// ── Corpus ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Format {
    Xmile,
    Vensim,
}

struct ModelSpec {
    key: &'static str,
    /// Path relative to CARGO_MANIFEST_DIR (src/simlin-engine).
    rel_path: &'static str,
    format: Format,
}

use Format::{Vensim, Xmile};

/// The curated corpus. Paths are relative to `CARGO_MANIFEST_DIR`
/// (`src/simlin-engine`); all 15 were verified to exist on disk.
const CORPUS: &[ModelSpec] = &[
    // canonical small
    ModelSpec {
        key: "teacup",
        rel_path: "../../test/test-models/samples/teacup/teacup.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "sir",
        rel_path: "../../test/test-models/samples/SIR/SIR.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "logistic_growth",
        rel_path: "../../test/logistic_growth_ltm/logistic_growth.stmx",
        format: Xmile,
    },
    // modules
    ModelSpec {
        key: "hares_and_foxes",
        rel_path: "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        format: Xmile,
    },
    // multipoint connectors
    ModelSpec {
        key: "multipoint",
        rel_path: "../../test/test-models/samples/display/multipoint-connection.stmx",
        format: Xmile,
    },
    // aliases
    ModelSpec {
        key: "alias1",
        rel_path: "../../test/alias1/alias1.stmx",
        format: Xmile,
    },
    // LTM / loop models
    ModelSpec {
        key: "cross_element",
        rel_path: "../../test/cross_element_ltm/cross_element.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "arrayed_pop",
        rel_path: "../../test/arrayed_population_ltm/arrayed_population.stmx",
        format: Xmile,
    },
    // ai-information reference set (human vs AI; used by Phase 4 calibration)
    ModelSpec {
        key: "ai_pure_human",
        rel_path: "../../test/ai-information/PureHumanModel.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_pure_ai",
        rel_path: "../../test/ai-information/PureAIModel.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_edited",
        rel_path: "../../test/ai-information/GeneratedByAIThenEdited.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_modules_arrays",
        rel_path: "../../test/ai-information/WithModulesAndArrays.stmx",
        format: Xmile,
    },
    // large metasd Vensim
    ModelSpec {
        key: "wrld3_03",
        rel_path: "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        format: Vensim,
    },
    ModelSpec {
        key: "beer_game",
        rel_path: "../../test/metasd/beer-game/RealBeer4-Sterman13.mdl",
        format: Vensim,
    },
    ModelSpec {
        key: "wonderland",
        rel_path: "../../test/metasd/wonderland/Wonderland3.mdl",
        format: Vensim,
    },
];

/// Resolve a corpus-relative path against the crate manifest dir.
fn abs_path(rel: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

/// Load one corpus model, dispatching on its declared format: XMILE through a
/// buffered reader + `open_xmile`, Vensim `.mdl` through a string + `open_vensim`
/// (mirrors `examples/backend_bench.rs`). Returns a human-readable error on any
/// I/O or parse failure so the caller can WARN-and-skip (AC3.6).
fn load_model(spec: &ModelSpec) -> Result<datamodel::Project, String> {
    let path = abs_path(spec.rel_path);
    match spec.format {
        Format::Xmile => {
            let file =
                std::fs::File::open(&path).map_err(|e| format!("failed to open {path}: {e}"))?;
            let mut reader = BufReader::new(file);
            open_xmile(&mut reader).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
        Format::Vensim => {
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {path}: {e}"))?;
            open_vensim(&contents).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
    }
}

/// Count the view elements in the model's as-loaded main view -- the diagram
/// the later tasks score and render. A model with no hand-authored view yields
/// 0 here (its layout is generated from scratch in Task 2).
fn loaded_element_count(project: &datamodel::Project) -> usize {
    let Some(model) = project.get_model(MAIN_MODEL) else {
        return 0;
    };
    match model.views.first() {
        Some(datamodel::View::StockFlow(sf)) => sf.elements.len(),
        None => 0,
    }
}

// ── Env knobs ────────────────────────────────────────────────────────────────

/// The set of corpus keys to run. `LAYOUT_EVAL_MODELS` is a comma list of keys;
/// unset/empty means the whole corpus. Unknown keys are reported and dropped so
/// a typo does not silently run nothing without explanation.
fn selected_keys() -> Vec<&'static str> {
    let Ok(raw) = env::var("LAYOUT_EVAL_MODELS") else {
        return CORPUS.iter().map(|s| s.key).collect();
    };
    let requested: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if requested.is_empty() {
        return CORPUS.iter().map(|s| s.key).collect();
    }
    let mut keys = Vec::new();
    for want in requested {
        match CORPUS.iter().find(|s| s.key == want) {
            Some(spec) => keys.push(spec.key),
            None => eprintln!("WARN: unknown model key {want:?}; skipping"),
        }
    }
    keys
}

/// Number of seeds M to sample per model (`LAYOUT_EVAL_SEEDS`, default 25).
fn seed_count() -> u64 {
    env::var("LAYOUT_EVAL_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SEEDS)
}

/// The seeds to sample: the union of the production best-of-k proxy
/// (`LAYOUT_SEEDS`) and `0..m`, deduped and sorted. Including `LAYOUT_SEEDS`
/// guarantees the best-of-k production proxy is always computable regardless of
/// `m`.
fn seed_set(m: u64) -> Vec<u64> {
    let mut seeds: BTreeSet<u64> = (0..m).collect();
    seeds.extend(LAYOUT_SEEDS);
    seeds.into_iter().collect()
}

/// The output directory (`LAYOUT_EVAL_OUT`, default repo-root
/// `target/layout-eval`, derived from `CARGO_MANIFEST_DIR`).
fn out_dir() -> String {
    env::var("LAYOUT_EVAL_OUT")
        .unwrap_or_else(|_| format!("{}/../../target/layout-eval", env!("CARGO_MANIFEST_DIR")))
}

fn main() {
    let keys = selected_keys();
    let m = seed_count();
    let seeds = seed_set(m);
    let out = out_dir();

    std::fs::create_dir_all(&out)
        .unwrap_or_else(|e| panic!("failed to create output dir {out}: {e}"));

    println!(
        "layout_eval: {} model(s), M={m} seeds (sampling {} unique), out={out}",
        keys.len(),
        seeds.len()
    );

    for spec in CORPUS.iter().filter(|s| keys.contains(&s.key)) {
        match load_model(spec) {
            Ok(project) => {
                let n = loaded_element_count(&project);
                println!("loaded {}: {n} elements", spec.key);
            }
            Err(err) => eprintln!("WARN: skipping {}: {err}", spec.key),
        }
    }
}
