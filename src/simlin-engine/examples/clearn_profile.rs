// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Standalone profiling harness for the C-LEARN hero model.
//!
//! Times each pipeline stage (parse, compile-via-salsa, VM construction, run)
//! and reports allocation counts / peak live bytes per stage via a counting
//! global allocator. Designed as a focused `perf record` / heaptrack target:
//! set `CLEARN_PROFILE=compile` or `CLEARN_PROFILE=run` and a high iteration
//! count to give an external sampler sustained signal on one stage.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example clearn_profile
//!   CLEARN_COMPILE_ITERS=20 CLEARN_PROFILE=compile \
//!     perf record -g -- target/release/examples/clearn_profile
//!   CLEARN_RUN_ITERS=200 CLEARN_PROFILE=run \
//!     perf record -g -- target/release/examples/clearn_profile
//!
//! Environment:
//!   CLEARN_MODEL          override the .mdl path
//!   CLEARN_LTM            "1" to compile with Loops That Matter enabled
//!   CLEARN_COMPILE_ITERS  extra compile-only iterations (default 0)
//!   CLEARN_RUN_ITERS      extra run-only iterations (default 0)
//!   CLEARN_PROFILE        "compile" | "run" | "both" (default both) -- which
//!                         extra-iteration loop(s) to execute
//!   CLEARN_COUNT_ALLOCS   "1" to count allocations per phase (distorts timing)
//!   CLEARN_ALLOC_HIST     "1" to also print, per phase, a histogram of
//!                         allocation and realloc sizes (implies counting)
//!   CLEARN_DIAGNOSTICS    "1" to run `collect_all_diagnostics` (the fragment
//!                         and unit passes) inside the compile phase, as every
//!                         product path that reports errors does
//!   CLEARN_RESIDENCY      "1" to report the bytes the database retains after
//!                         a compile, with the artifact, the database and the
//!                         sync state dropped one at a time

use std::alloc::{GlobalAlloc, Layout};

// Back the counting allocator with mimalloc, which is what every native binary
// embedding the engine installs (simlin-cli, simlin-serve, simlin-mcp, and
// libsimlin under its `mimalloc` feature, which pysimlin's build turns on).
// The compile path is allocation-bound, so profiling against system malloc
// measures an allocator no shipped native build runs.
use mimalloc::MiMalloc as Backing;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use simlin_engine::db::{
    PersistentSyncState, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::{CompiledSimulation, Vm, open_vensim};

// --- Counting allocator -----------------------------------------------------
//
// Tracks cumulative allocation calls/bytes plus live bytes and a high-water
// mark. A `GlobalAlloc` must be `Sync` and serves every thread in the process,
// so the counters are atomic and the peak is maintained with a CAS loop. That
// is a requirement of the allocator position, not of the workload:
// compile_project_incremental runs on one thread today (measured at 0.9996 CPUs
// utilized). `realloc` is overridden so reallocs can be histogrammed
// separately (an allocator serves a grow-in-place very differently from a
// fresh block), but each one is still counted as one allocation of the new
// size, exactly as the default `GlobalAlloc::realloc` (alloc + copy + dealloc)
// would count it, so the `allocs` column does not depend on the override.

struct Counting;

static COUNTING_ON: AtomicBool = AtomicBool::new(false);
static HIST_ON: AtomicBool = AtomicBool::new(false);
static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static REALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

// Size-class histogram (CLEARN_ALLOC_HIST=1). Bins are 8 bytes wide up to
// 1 KiB, the range a size-class allocator serves from per-class pages, and one
// bin per power of two above that. Reallocs are binned by their NEW size.
const HIST_SMALL_BINS: usize = 128;
const HIST_LARGE_BINS: usize = 22;
const HIST_BINS: usize = HIST_SMALL_BINS + HIST_LARGE_BINS;
static HIST_ALLOCS: [AtomicUsize; HIST_BINS] = [const { AtomicUsize::new(0) }; HIST_BINS];
static HIST_BYTES: [AtomicUsize; HIST_BINS] = [const { AtomicUsize::new(0) }; HIST_BINS];
static HIST_REALLOCS: [AtomicUsize; HIST_BINS] = [const { AtomicUsize::new(0) }; HIST_BINS];

fn hist_bin(size: usize) -> usize {
    if size <= 8 * HIST_SMALL_BINS {
        // 1..=8 -> 0, 9..=16 -> 1, ...; a zero-size request never reaches
        // GlobalAlloc, but clamp anyway.
        (size.max(1) - 1) / 8
    } else {
        // ceil(log2(size)): 1025..=2048 -> 11, 2049..=4096 -> 12, ...
        let log2 = (usize::BITS - (size - 1).leading_zeros()) as usize;
        HIST_SMALL_BINS + (log2 - 11).min(HIST_LARGE_BINS - 1)
    }
}

/// The inclusive upper size bound of a histogram bin, for printing.
fn hist_bin_upper(bin: usize) -> usize {
    if bin < HIST_SMALL_BINS {
        8 * (bin + 1)
    } else {
        1usize << (11 + bin - HIST_SMALL_BINS)
    }
}

fn record_live(delta: isize) {
    let live = if delta >= 0 {
        LIVE_BYTES.fetch_add(delta as usize, Ordering::Relaxed) + delta as usize
    } else {
        LIVE_BYTES.fetch_sub(delta.unsigned_abs(), Ordering::Relaxed) - delta.unsigned_abs()
    };
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

fn record_alloc(size: usize, realloc: bool) {
    ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
    ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
    if HIST_ON.load(Ordering::Relaxed) {
        let bin = hist_bin(size);
        HIST_ALLOCS[bin].fetch_add(1, Ordering::Relaxed);
        HIST_BYTES[bin].fetch_add(size, Ordering::Relaxed);
        if realloc {
            HIST_REALLOCS[bin].fetch_add(1, Ordering::Relaxed);
        }
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { Backing.alloc(layout) };
        // Counting is gated so the default run measures true wall-clock without
        // per-allocation atomic overhead. Enable with CLEARN_COUNT_ALLOCS=1 to
        // get allocation counts (at the cost of distorted timing).
        if !p.is_null() && COUNTING_ON.load(Ordering::Relaxed) {
            record_alloc(layout.size(), false);
            record_live(layout.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { Backing.dealloc(ptr, layout) };
        if COUNTING_ON.load(Ordering::Relaxed) {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { Backing.realloc(ptr, layout, new_size) };
        if !p.is_null() && COUNTING_ON.load(Ordering::Relaxed) {
            REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            record_alloc(new_size, true);
            record_live(new_size as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy)]
struct Snap {
    calls: usize,
    bytes: usize,
    reallocs: usize,
    live: usize,
}

fn snap() -> Snap {
    Snap {
        calls: ALLOC_CALLS.load(Ordering::Relaxed),
        bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        reallocs: REALLOC_CALLS.load(Ordering::Relaxed),
        live: LIVE_BYTES.load(Ordering::Relaxed),
    }
}

/// The histogram counters at one instant.
struct HistSnap {
    allocs: [usize; HIST_BINS],
    bytes: [usize; HIST_BINS],
    reallocs: [usize; HIST_BINS],
}

fn hist_snap() -> HistSnap {
    let load = |counters: &[AtomicUsize; HIST_BINS]| {
        let mut out = [0usize; HIST_BINS];
        for (slot, counter) in out.iter_mut().zip(counters) {
            *slot = counter.load(Ordering::Relaxed);
        }
        out
    };
    HistSnap {
        allocs: load(&HIST_ALLOCS),
        bytes: load(&HIST_BYTES),
        reallocs: load(&HIST_REALLOCS),
    }
}

/// Print the non-empty bins of the histogram delta between two snapshots:
/// allocation count (with its share of the phase's allocations and the running
/// cumulative share), bytes requested, and how many of the bin's allocations
/// arrived as reallocs.
fn print_hist_delta(before: &HistSnap, after: &HistSnap) {
    let total: usize = (0..HIST_BINS)
        .map(|bin| after.allocs[bin] - before.allocs[bin])
        .sum();
    if total == 0 {
        return;
    }
    println!("  size histogram (bin upper bound, inclusive):");
    println!(
        "    {:>12} {:>10} {:>6} {:>6} {:>10} {:>9}",
        "size <=", "allocs", "%", "cum%", "MiB", "reallocs"
    );
    let mut cum = 0usize;
    for bin in 0..HIST_BINS {
        let allocs = after.allocs[bin] - before.allocs[bin];
        if allocs == 0 {
            continue;
        }
        cum += allocs;
        println!(
            "    {:>12} {:>10} {:>6.2} {:>6.2} {:>10.2} {:>9}",
            hist_bin_upper(bin),
            allocs,
            100.0 * allocs as f64 / total as f64,
            100.0 * cum as f64 / total as f64,
            mib(after.bytes[bin] - before.bytes[bin]),
            after.reallocs[bin] - before.reallocs[bin],
        );
    }
}

/// Reset the peak high-water mark to the current live bytes so the next phase's
/// peak is measured relative to its own starting point.
fn reset_peak() {
    PEAK_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Run `f` as a measured phase: report wall time, allocation calls/bytes during
/// the phase, net retained (live) bytes, and peak live bytes reached.
fn phase<T>(name: &str, f: impl FnOnce() -> T) -> T {
    reset_peak();
    let hist_before = HIST_ON.load(Ordering::Relaxed).then(hist_snap);
    let before = snap();
    let t0 = Instant::now();
    let out = f();
    let elapsed = t0.elapsed();
    let after = snap();
    let peak = PEAK_BYTES.load(Ordering::Relaxed);

    let calls = after.calls - before.calls;
    let bytes = after.bytes - before.bytes;
    let reallocs = after.reallocs - before.reallocs;
    let retained = after.live as i64 - before.live as i64;

    println!(
        "{name:<22} {:>9.2} ms | allocs {:>10} | alloc'd {:>9.1} MiB | retained {:>+8.1} MiB | peak {:>8.1} MiB | reallocs {:>8}",
        elapsed.as_secs_f64() * 1000.0,
        calls,
        mib(bytes),
        retained as f64 / (1024.0 * 1024.0),
        mib(peak),
        reallocs,
    );
    if let Some(hist_before) = hist_before {
        print_hist_delta(&hist_before, &hist_snap());
    }
    out
}

fn model_path() -> String {
    if let Ok(p) = std::env::var("CLEARN_MODEL") {
        return p;
    }
    format!(
        "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// One compile with everything it retains: the database (every salsa memo),
/// the sync state (the input handles) and the artifact.
struct RetainedCompile {
    db: SimlinDb,
    sync: PersistentSyncState,
    compiled: std::sync::Arc<CompiledSimulation>,
}

fn compile_retained(
    datamodel: &simlin_engine::datamodel::Project,
    ltm: bool,
    diagnostics: bool,
) -> RetainedCompile {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    if ltm {
        set_project_ltm_enabled(&mut db, sync.project, true);
    }
    if diagnostics {
        std::hint::black_box(collect_all_diagnostics(&db, sync.project));
    }
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    RetainedCompile { db, sync, compiled }
}

fn compile_once(
    datamodel: &simlin_engine::datamodel::Project,
    ltm: bool,
    diagnostics: bool,
) -> std::sync::Arc<CompiledSimulation> {
    compile_retained(datamodel, ltm, diagnostics).compiled
}

fn print_residency(label: &str, baseline: usize) {
    let live = LIVE_BYTES.load(Ordering::Relaxed);
    let delta = live as i64 - baseline as i64;
    println!(
        "residency {label:<18} live {:>12} bytes ({:>8.2} MiB) | above baseline {delta:>+12} bytes ({:>+8.2} MiB)",
        live,
        mib(live),
        delta as f64 / (1024.0 * 1024.0),
    );
}

/// What a compile leaves resident, attributed by dropping each owner in turn:
/// the artifact first (what a caller keeps to simulate), then the database
/// (every salsa memo -- the parse and lowering memos, fragments, layouts),
/// then the sync state. The parsed datamodel stays alive throughout, as it
/// does in every embedding.
fn residency_census(datamodel: &simlin_engine::datamodel::Project, ltm: bool, diagnostics: bool) {
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    print_residency("baseline", baseline);
    let retained = compile_retained(datamodel, ltm, diagnostics);
    print_residency("db+sync+artifact", baseline);
    let RetainedCompile { db, sync, compiled } = retained;
    drop(compiled);
    print_residency("db+sync", baseline);
    drop(db);
    print_residency("sync", baseline);
    drop(sync);
    print_residency("after all drops", baseline);
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let path = model_path();
    let compile_iters = env_usize("CLEARN_COMPILE_ITERS", 0);
    let run_iters = env_usize("CLEARN_RUN_ITERS", 0);
    let which = std::env::var("CLEARN_PROFILE").unwrap_or_else(|_| "both".to_string());
    let ltm = std::env::var("CLEARN_LTM").is_ok_and(|v| v != "0");
    let diagnostics = std::env::var("CLEARN_DIAGNOSTICS").is_ok_and(|v| v != "0");
    let residency = std::env::var("CLEARN_RESIDENCY").is_ok_and(|v| v != "0");
    if residency || std::env::var("CLEARN_COUNT_ALLOCS").is_ok_and(|v| v != "0") {
        COUNTING_ON.store(true, Ordering::Relaxed);
    }
    if std::env::var("CLEARN_ALLOC_HIST").is_ok_and(|v| v != "0") {
        COUNTING_ON.store(true, Ordering::Relaxed);
        HIST_ON.store(true, Ordering::Relaxed);
    }

    println!("model: {path}");
    println!("ltm:   {ltm}");
    println!("diagnostics: {diagnostics}");

    let contents = phase("read_file", || {
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
    });
    println!(
        "  source: {} bytes, {} lines",
        contents.len(),
        contents.lines().count()
    );

    let datamodel = phase("parse (open_vensim)", || open_vensim(&contents).unwrap());
    let n_models = datamodel.models.len();
    let n_vars: usize = datamodel.models.iter().map(|m| m.variables.len()).sum();
    println!(
        "  models: {n_models}, datamodel variables: {n_vars}, dims: {}",
        datamodel.dimensions.len()
    );

    if residency {
        residency_census(&datamodel, ltm, diagnostics);
    }

    let compiled = phase("compile (salsa)", || {
        compile_once(&datamodel, ltm, diagnostics)
    });
    println!("  n_slots (root): {}", compiled.n_slots());

    let prof = compiled.bytecode_profile();
    println!(
        "  bytecode: {} opcodes ({:.1} KiB @ 8B) = {} flow + {} stock + {} initial ({} initials)",
        prof.total_opcodes,
        (prof.total_opcodes * 8) as f64 / 1024.0,
        prof.flow_opcodes,
        prof.stock_opcodes,
        prof.initial_opcodes,
        prof.n_initials,
    );
    println!(
        "  flow opcodes after 3-address fusion (est): {} -> {} ({:.1}% reduction)",
        prof.flow_opcodes,
        prof.flow_opcodes_after_fusion,
        100.0 * (prof.flow_opcodes - prof.flow_opcodes_after_fusion) as f64
            / prof.flow_opcodes as f64,
    );
    println!(
        "  tables: {} literals, {} GFs / {} points, {} temp slots, {} dims, {} static_views, {} dim_lists, {} names, {} modules",
        prof.total_literals,
        prof.graphical_functions,
        prof.graphical_function_points,
        prof.temp_storage_slots,
        prof.dimensions,
        prof.static_views,
        prof.dim_lists,
        prof.names,
        prof.n_modules,
    );
    let mut hist: Vec<_> = prof.histogram.iter().collect();
    hist.sort_by(|a, b| b.1.cmp(a.1));
    // `CLEARN_HISTOGRAM=full` prints every opcode rather than the top 25, so a
    // ledger row can account for a rare opcode's count (the array-producing
    // builtins are far outside the top 25 on C-LEARN).
    let shown = if std::env::var("CLEARN_HISTOGRAM").is_ok_and(|v| v == "full") {
        hist.len()
    } else {
        25
    };
    println!("  opcode histogram ({shown} of {}):", prof.histogram.len());
    for (name, count) in hist.iter().take(shown) {
        let pct = **count as f64 / prof.total_opcodes as f64 * 100.0;
        println!("    {name:<22} {count:>9}  {pct:>5.1}%");
    }

    let fused_total: usize = prof.fused_histogram.values().sum();
    let mut fhist: Vec<_> = prof.fused_histogram.iter().collect();
    fhist.sort_by(|a, b| b.1.cmp(a.1));
    println!("  post-fusion flow+stock stream: {fused_total} opcodes; fused-binop counts:");
    for (name, count) in fhist
        .iter()
        .filter(|(n, _)| n.starts_with("Bin") || n.starts_with("Assign"))
    {
        println!("    {name:<22} {count:>9}");
    }

    let mut vm = phase("Vm::new", || Vm::new(compiled.clone()).unwrap());
    println!("  variables (offsets): {}", vm.names_as_strs().len());

    phase("run_to_end", || vm.run_to_end().unwrap());
    let results = vm.into_results();
    println!(
        "  result slots/step: {}, saved steps: {}",
        results.step_size, results.step_count
    );

    // Extra-iteration loops for external samplers (perf/heaptrack). Kept out of
    // the per-phase accounting above; these print only aggregate timing.
    let do_compile = which == "both" || which == "compile";
    let do_run = which == "both" || which == "run";

    if compile_iters > 0 && do_compile {
        let t0 = Instant::now();
        for _ in 0..compile_iters {
            std::hint::black_box(compile_once(&datamodel, ltm, diagnostics));
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / compile_iters as f64;
        println!("compile x{compile_iters}: {per:.2} ms/iter");
    }

    if run_iters > 0 && do_run {
        let compiled = compile_once(&datamodel, ltm, diagnostics);
        let t0 = Instant::now();
        for _ in 0..run_iters {
            let mut vm = Vm::new(compiled.clone()).unwrap();
            vm.run_to_end().unwrap();
            std::hint::black_box(&vm);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / run_iters as f64;
        println!("run x{run_iters}: {per:.2} ms/iter (incl. Vm::new + clone)");
    }
}
