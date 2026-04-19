// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Full-path LTM compile benchmark.
//!
//! Usage: `cargo run --release --example ltm_full_bench -- [mdl]`
//!
//! Example:
//!   cargo run --release --example ltm_full_bench -- \
//!       test/metasd/WRLD3-03/wrld3-03.mdl
//!
//! Unlike `ltm_mem_bench` (which stops after circuit enumeration), this
//! harness drives the full salsa-backed LTM pipeline end-to-end so we
//! can profile each stage's wall time and memory footprint.  The
//! [`MAX_LTM_CIRCUITS`]-based circuit cap has been retired in favour of
//! the auto-flip gate (see
//! `docs/design-plans/2026-04-18-ltm-cap-lift-validation.md`); this
//! bench measures the post-cap pipeline with the real production
//! parameters -- enumeration runs uncapped and the downstream
//! synthetic-variable pipeline is gated by
//! `simlin_engine::ltm::MAX_LTM_TOTAL_CIRCUITS`.
//!
//! The stages are measured in order, with cumulative VmPeak / VmHWM /
//! VmRSS (all from /proc/self/status) plus wall-clock time recorded at
//! each step:
//!
//!   1. parsed              -- XMILE/MDL -> datamodel::Project
//!   2. synced              -- sync_from_datamodel_incremental
//!   3. ltm_enabled         -- set_project_ltm_enabled
//!   4. causal_edges        -- model_causal_edges (variable-level)
//!   5. element_edges       -- model_element_causal_edges
//!   6. loop_circuits       -- model_element_loop_circuits (Johnson's)
//!   7. ltm_variables       -- model_ltm_variables (synth var gen)
//!   8. compile             -- compile_project_incremental (full assembly)

use std::fs;
use std::time::Instant;

use simlin_engine::db::{
    SimlinDb, compile_project_incremental, model_causal_edges, model_element_causal_edges,
    model_element_loop_circuits, model_ltm_variables, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::{open_vensim, open_xmile};

fn read_proc_status_kb(key: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches(':').trim();
            let value = rest.trim_end_matches(" kB").trim();
            return value.parse().ok();
        }
    }
    None
}

fn mib(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

#[derive(Default, Clone)]
struct MemSnapshot {
    peak_kb: u64,
    hwm_kb: u64,
    rss_kb: u64,
}

fn snapshot() -> MemSnapshot {
    MemSnapshot {
        peak_kb: read_proc_status_kb("VmPeak").unwrap_or(0),
        hwm_kb: read_proc_status_kb("VmHWM").unwrap_or(0),
        rss_kb: read_proc_status_kb("VmRSS").unwrap_or(0),
    }
}

struct StageRecord {
    label: &'static str,
    wall_ms: f64,
    peak_mib: f64,
    hwm_mib: f64,
    rss_mib: f64,
    /// VmPeak delta from the previous stage (MiB).  Isolates per-stage
    /// peak growth even though VmPeak is cumulative.
    delta_peak_mib: f64,
    /// VmRSS delta from the previous stage (MiB).  Negative values
    /// indicate allocator returned memory during the stage.
    delta_rss_mib: f64,
    note: String,
}

fn fmt_header() -> String {
    format!(
        "{:<22}  {:>9}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {}",
        "stage", "wall(ms)", "VmPeak", "dPeak", "VmHWM", "VmRSS", "dRSS", "note"
    )
}

fn fmt_record(r: &StageRecord) -> String {
    format!(
        "{:<22}  {:>9.1}  {:>10.2}  {:>+10.2}  {:>10.2}  {:>10.2}  {:>+10.2}  {}",
        r.label,
        r.wall_ms,
        r.peak_mib,
        r.delta_peak_mib,
        r.hwm_mib,
        r.rss_mib,
        r.delta_rss_mib,
        r.note,
    )
}

struct Tracker {
    records: Vec<StageRecord>,
    last: MemSnapshot,
    /// Hard memory ceiling (MiB) -- if VmPeak crosses this we abort.
    abort_peak_mib: f64,
}

impl Tracker {
    fn new(initial: MemSnapshot, abort_peak_mib: f64) -> Self {
        Self {
            records: Vec::new(),
            last: initial,
            abort_peak_mib,
        }
    }

    fn record(&mut self, label: &'static str, wall_ms: f64, note: String) {
        let now = snapshot();
        let record = StageRecord {
            label,
            wall_ms,
            peak_mib: mib(now.peak_kb),
            hwm_mib: mib(now.hwm_kb),
            rss_mib: mib(now.rss_kb),
            delta_peak_mib: mib(now.peak_kb) - mib(self.last.peak_kb),
            delta_rss_mib: mib(now.rss_kb) - mib(self.last.rss_kb),
            note,
        };
        eprintln!("{}", fmt_record(&record));
        self.last = now;
        self.records.push(record);
        if self.last.peak_kb as f64 / 1024.0 > self.abort_peak_mib {
            eprintln!(
                "\n!!! VmPeak crossed abort ceiling {:.0} MiB -- aborting before the next stage",
                self.abort_peak_mib
            );
            std::process::exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mdl_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "test/metasd/WRLD3-03/wrld3-03.mdl".to_string());

    // Abort ceiling: 15 GiB by default; override with LTM_BENCH_ABORT_MIB.
    let abort_peak_mib: f64 = std::env::var("LTM_BENCH_ABORT_MIB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15.0 * 1024.0);

    eprintln!("=== LTM full-pipeline benchmark ===");
    eprintln!("model:       {mdl_path}");
    eprintln!("abort @:     {:.0} MiB VmPeak", abort_peak_mib);
    eprintln!();

    let initial = snapshot();
    eprintln!(
        "startup VmPeak={:.2} MiB  VmHWM={:.2} MiB  VmRSS={:.2} MiB",
        mib(initial.peak_kb),
        mib(initial.hwm_kb),
        mib(initial.rss_kb),
    );
    eprintln!();
    eprintln!("{}", fmt_header());

    let mut tracker = Tracker::new(initial, abort_peak_mib);
    let run_start = Instant::now();

    // Stage 1: parse model file.  MDL and XMILE (.stmx/.xmile) are both
    // supported so the bench can profile non-Vensim test models without
    // needing per-format driver scripts.
    let t0 = Instant::now();
    let contents = fs::read_to_string(&mdl_path).expect("read model file");
    let datamodel = if mdl_path.ends_with(".mdl") {
        open_vensim(&contents).expect("parse MDL")
    } else {
        let mut reader = contents.as_bytes();
        open_xmile(&mut reader).expect("parse XMILE")
    };
    let n_models = datamodel.models.len();
    let n_root_vars = datamodel
        .models
        .first()
        .map(|m| m.variables.len())
        .unwrap_or(0);
    tracker.record(
        "parsed",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!("{n_models} models, root_vars={n_root_vars}"),
    );

    // Stage 2: sync into a fresh salsa database.
    let mut db = SimlinDb::default();
    let t0 = Instant::now();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let root_name = datamodel
        .models
        .first()
        .map(|m| m.name.as_str())
        .expect("datamodel must contain at least one model");
    let root_canonical = simlin_engine::canonicalize(root_name).into_owned();
    let root_source_model = sync
        .models
        .get(&root_canonical)
        .expect("root model must be present in sync result")
        .source_model;
    tracker.record(
        "synced",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!("root='{root_name}'"),
    );

    // Stage 3: enable LTM.  Just flips an input flag but bumps the
    // salsa generation, so downstream tracked fns will run fresh.
    let t0 = Instant::now();
    set_project_ltm_enabled(&mut db, sync.project, true);
    tracker.record(
        "ltm_enabled",
        t0.elapsed().as_secs_f64() * 1000.0,
        "flag=true".into(),
    );

    // Stage 4: variable-level causal edges.
    let t0 = Instant::now();
    let edges = model_causal_edges(&db, root_source_model, sync.project);
    let n_edge_src = edges.edges.len();
    let n_edge_total: usize = edges.edges.values().map(|v| v.len()).sum();
    let n_stocks = edges.stocks.len();
    tracker.record(
        "causal_edges",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!("src_nodes={n_edge_src} total_edges={n_edge_total} stocks={n_stocks}"),
    );

    // Stage 5: element-level causal edges (A2A / cross-element expansion).
    let t0 = Instant::now();
    let elem_edges = model_element_causal_edges(&db, root_source_model, sync.project);
    let n_elem_src = elem_edges.edges.len();
    let n_elem_total: usize = elem_edges.edges.values().map(|v| v.len()).sum();
    let n_elem_stocks = elem_edges.stocks.len();
    tracker.record(
        "element_edges",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!("src_nodes={n_elem_src} total_edges={n_elem_total} stocks={n_elem_stocks}"),
    );

    // Stage 6: element-level circuit enumeration (Johnson's w/ SCC).
    // Enumeration is bounded by the streaming MAX_LTM_ENUMERATION_CAP
    // inside `model_element_loop_circuits`; the downstream
    // synthetic-variable pipeline is gated by MAX_LTM_TOTAL_CIRCUITS
    // in `model_ltm_variables`.
    let t0 = Instant::now();
    let circuits_result = model_element_loop_circuits(&db, root_source_model, sync.project);
    let n_circuits = circuits_result.len();
    let n_circuit_names = circuits_result.names.len();
    tracker.record(
        "loop_circuits",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!("circuits={n_circuits} unique_names={n_circuit_names}"),
    );

    // Stage 7: LTM synthetic variables (link scores, loop scores,
    // pathways, composites).  Relative loop scores are computed
    // post-simulation via `ltm_post::compute_rel_loop_scores`, so they
    // no longer contribute to this stage's equation-text footprint.
    let t0 = Instant::now();
    let ltm_vars = model_ltm_variables(&db, root_source_model, sync.project);
    let n_ltm = ltm_vars.vars.len();
    let mut n_link = 0usize;
    let mut n_loop = 0usize;
    let mut n_path = 0usize;
    let mut n_comp = 0usize;
    let mut n_a2a = 0usize;
    for v in &ltm_vars.vars {
        if v.name.contains("\u{205A}link_score\u{205A}") {
            n_link += 1;
        } else if v.name.contains("\u{205A}loop_score\u{205A}") {
            n_loop += 1;
        } else if v.name.contains("\u{205A}path\u{205A}") {
            n_path += 1;
        } else if v.name.contains("\u{205A}composite\u{205A}") {
            n_comp += 1;
        }
        if !v.dimensions.is_empty() {
            n_a2a += 1;
        }
    }
    let total_eqn_bytes: usize = ltm_vars.vars.iter().map(|v| v.equation.len()).sum();
    tracker.record(
        "ltm_variables",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!(
            "ltm_vars={n_ltm} (link={n_link} loop={n_loop} path={n_path} \
             comp={n_comp} a2a={n_a2a})  eq_bytes={total_eqn_bytes}"
        ),
    );

    // Stage 8: full compile (parsing LTM equations into ASTs, interning
    // salsa rows, bytecode emission).
    let t0 = Instant::now();
    let compile_result = compile_project_incremental(&db, sync.project, root_name);
    let compile_ok = compile_result.is_ok();
    tracker.record(
        "compile",
        t0.elapsed().as_secs_f64() * 1000.0,
        format!(
            "ok={} {}",
            compile_ok,
            match &compile_result {
                Ok(_) => String::new(),
                Err(e) => format!("err={:?}", e),
            }
        ),
    );

    let total_ms = run_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!();
    eprintln!(
        "TOTAL wall={:.1}ms  final VmPeak={:.2} MiB  final VmHWM={:.2} MiB  final VmRSS={:.2} MiB",
        total_ms,
        tracker.records.last().map(|r| r.peak_mib).unwrap_or(0.0),
        tracker.records.last().map(|r| r.hwm_mib).unwrap_or(0.0),
        tracker.records.last().map(|r| r.rss_mib).unwrap_or(0.0),
    );

    // Emit a machine-readable summary line as the last line of stderr
    // so the driver shell script can scrape it without regexing the
    // whole table.  Format:
    //   SUMMARY circuits=N vars=N peak_mib=F hwm_mib=F rss_mib=F wall_ms=F compile_ok=BOOL
    eprintln!(
        "SUMMARY circuits={} vars={} peak_mib={:.2} hwm_mib={:.2} rss_mib={:.2} \
         wall_ms={:.1} compile_ok={}",
        n_circuits,
        n_ltm,
        tracker.records.last().map(|r| r.peak_mib).unwrap_or(0.0),
        tracker.records.last().map(|r| r.hwm_mib).unwrap_or(0.0),
        tracker.records.last().map(|r| r.rss_mib).unwrap_or(0.0),
        total_ms,
        compile_ok,
    );
}
