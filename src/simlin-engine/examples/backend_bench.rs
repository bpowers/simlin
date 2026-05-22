// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Backend comparison benchmark: the bytecode VM vs the wasm backend.
//!
//! Two execution paths take the *same* salsa-compiled `CompiledSimulation` and
//! run it:
//!   - the hand-rolled bytecode VM interpreter (`Vm::new` + `run_to_end`); and
//!   - the wasm code-generation backend (`compile_simulation`) executed under the
//!     pure-Rust DLR-FT `wasm-interpreter`.
//!
//! Both sides are *interpreters* (no JIT on either). That is deliberate: it
//! isolates the question "is lowering our bytecode to wasm and running it under
//! a generic wasm interpreter competitive with our purpose-built VM?" from the
//! separate question of what a JIT (e.g. browser V8) would buy later. It also
//! lets a single counting global allocator measure memory uniformly across both
//! backends -- the VM's slabs, the wasm codegen, the interpreter's own state,
//! and the wasm linear-memory slab are all native Rust heap allocations on a
//! level field, with no native-vs-wasm accounting mismatch.
//!
//! For each model it reports, with a time pass (counting allocator OFF, for true
//! wall-clock) and a memory pass (counting allocator ON):
//!   - the shared front-end compile (`compile_project_incremental`), common to
//!     both backends and measured once;
//!   - backend build: VM = `Vm::new`; wasm = `compile_simulation` + `validate` +
//!     instantiate;
//!   - eval / re-run latency: VM = `reset` + `run_to_end`; wasm = `run` -- both
//!     excluding build/instantiate, i.e. the interactive "scrub a slider, re-run"
//!     cost the wasm backend was built for; and
//!   - the emitted wasm blob size.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example backend_bench
//!   BENCH_MODELS=fishbanks,wrld3 cargo run --release -p simlin-engine --example backend_bench
//!   BENCH_TIME_BUDGET=5 cargo run ...   # per-phase wall-clock budget, seconds
//!
//! `BENCH_MODELS` is a comma list drawn from {fishbanks, wrld3, clearn};
//! default is all three. C-LEARN's wasm run under the DLR-FT interpreter is the
//! slow case (a large model under a non-JIT wasm interpreter); the adaptive
//! budget falls back to a single iteration for any phase that exceeds it.

use std::alloc::{GlobalAlloc, Layout, System as Backing};
use std::hint::black_box;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use checked::Store;
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::wasmgen::{WasmArtifact, compile_simulation};
use simlin_engine::{CompiledSimulation, Vm, datamodel, open_vensim, open_xmile};
use wasm::validate;

// ── Counting allocator ──────────────────────────────────────────────────────
//
// Mirrors `examples/clearn_profile.rs`: cumulative alloc calls/bytes plus live
// bytes and a high-water peak, all atomic (compile fans out across rayon). The
// time pass leaves counting OFF so the per-allocation atomics don't distort
// wall-clock; the memory pass turns it ON. The default `GlobalAlloc::realloc`
// routes through alloc/dealloc, so realloc is counted without an override.

struct Counting;

static COUNTING_ON: AtomicBool = AtomicBool::new(false);
static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { Backing.alloc(layout) };
        if !p.is_null() && COUNTING_ON.load(Ordering::Relaxed) {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
            while live > peak {
                match PEAK_BYTES.compare_exchange_weak(
                    peak,
                    live,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => peak = observed,
                }
            }
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { Backing.dealloc(ptr, layout) };
        if COUNTING_ON.load(Ordering::Relaxed) {
            // saturating: a free of memory allocated before counting was enabled
            // (or before a counter reset) must not underflow-wrap the live total.
            let size = layout.size();
            let mut cur = LIVE_BYTES.load(Ordering::Relaxed);
            loop {
                let next = cur.saturating_sub(size);
                match LIVE_BYTES.compare_exchange_weak(
                    cur,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => cur = observed,
                }
            }
        }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy)]
struct Snap {
    calls: usize,
    bytes: usize,
    live: usize,
}

fn snap() -> Snap {
    Snap {
        calls: ALLOC_CALLS.load(Ordering::Relaxed),
        bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        live: LIVE_BYTES.load(Ordering::Relaxed),
    }
}

fn reset_peak() {
    PEAK_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// One phase's memory footprint, captured between two `snap`s with the peak
/// high-water mark reset at the start of the phase.
#[derive(Clone, Copy, Default)]
struct Mem {
    calls: usize,
    alloc_mib: f64,
    retained_mib: f64,
    peak_mib: f64,
}

fn mem_between(before: Snap, after: Snap, peak_bytes: usize) -> Mem {
    Mem {
        calls: after.calls - before.calls,
        alloc_mib: mib(after.bytes - before.bytes),
        retained_mib: (after.live as i64 - before.live as i64) as f64 / (1024.0 * 1024.0),
        peak_mib: mib(peak_bytes),
    }
}

// ── Timing ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Stat {
    median_ms: f64,
    min_ms: f64,
    iters: usize,
}

impl Stat {
    fn from_times(mut times: Vec<f64>) -> Stat {
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let iters = times.len();
        let min_ms = times.first().copied().unwrap_or(f64::NAN);
        let median_ms = if iters == 0 {
            f64::NAN
        } else {
            times[iters / 2]
        };
        Stat {
            median_ms,
            min_ms,
            iters,
        }
    }
}

/// Run `body` (which returns the elapsed ms of just its *timed* region, doing
/// any setup untimed) at least `min_iters` times, then until `max_iters` or
/// `budget_s` wall-clock elapses. Reporting median + min + iters makes the
/// sample size visible -- a one-iteration heavy phase is not silently averaged.
fn bench(min_iters: usize, max_iters: usize, budget_s: f64, mut body: impl FnMut() -> f64) -> Stat {
    let mut times = Vec::new();
    let start = Instant::now();
    while times.len() < max_iters
        && (times.len() < min_iters || start.elapsed().as_secs_f64() < budget_s)
    {
        times.push(body());
    }
    Stat::from_times(times)
}

fn ms_since(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

/// Format a duration in milliseconds with adaptive precision so values spanning
/// sub-microsecond (`Vm::new` on a tiny model) to seconds (C-LEARN compile) are
/// all legible in the same column. Always milliseconds; only the precision
/// varies.
fn fmt_ms(v: f64) -> String {
    if !v.is_finite() {
        "-".to_string()
    } else if v >= 100.0 {
        format!("{v:.1}")
    } else if v >= 1.0 {
        format!("{v:.3}")
    } else if v >= 0.001 {
        format!("{v:.4}")
    } else {
        format!("{v:.6}")
    }
}

// ── Model loading + the shared front-end compile ──────────────────────────────

struct ModelSpec {
    key: &'static str,
    label: &'static str,
    /// Path relative to CARGO_MANIFEST_DIR (src/simlin-engine).
    rel_path: &'static str,
    format: Format,
}

#[derive(Clone, Copy)]
enum Format {
    Xmile,
    Vensim,
}

const MODELS: &[ModelSpec] = &[
    ModelSpec {
        key: "fishbanks",
        label: "fishbanks",
        rel_path: "../../default_projects/fishbanks/model.xmile",
        format: Format::Xmile,
    },
    ModelSpec {
        key: "wrld3",
        label: "WORLD3-03",
        rel_path: "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        format: Format::Vensim,
    },
    ModelSpec {
        key: "clearn",
        label: "C-LEARN v77",
        rel_path: "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        format: Format::Vensim,
    },
];

fn abs_path(rel: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

fn load_datamodel(spec: &ModelSpec) -> datamodel::Project {
    let path = abs_path(spec.rel_path);
    match spec.format {
        Format::Xmile => {
            let file =
                std::fs::File::open(&path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
            let mut reader = BufReader::new(file);
            open_xmile(&mut reader).unwrap_or_else(|e| panic!("failed to parse {path}: {e:?}"))
        }
        Format::Vensim => {
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
            open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e:?}"))
        }
    }
}

/// The front-end compile, identical for both backends: datamodel -> salsa db ->
/// `CompiledSimulation` (the value `Vm::new` and `compile_simulation` consume).
fn front_end_compile(datamodel: &datamodel::Project) -> CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    compile_project_incremental(&db, sync.project, "main").expect("incremental compile")
}

// ── wasm instantiation under the DLR-FT interpreter ───────────────────────────
//
// `validate(&wasm)` borrows the blob, so the caller must keep the `WasmArtifact`
// alive for the lifetime of the returned info/store. The blob is self-contained
// (no host imports), so instantiation passes an empty import vector.

/// Run the artifact once under the interpreter and return the whole step-major
/// results slab. Used only for the correctness cross-check, not in timed loops.
fn run_artifact_slab(artifact: &WasmArtifact) -> Vec<f64> {
    let info = validate(&artifact.wasm).expect("generated module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run = store
        .instance_export(inst, "run")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run wasm");
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let n = artifact.layout.n_chunks * artifact.layout.n_slots;
    let base = artifact.layout.results_offset;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..n)
            .map(|i| {
                let a = base + i * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Confirm both backends compute the same simulation, so the timings describe a
/// real, correct run. Compares every layout variable's full series VM-vs-wasm
/// and returns (variables_checked, max_abs_diff).
fn cross_check(compiled: &CompiledSimulation, artifact: &WasmArtifact) -> (usize, f64) {
    let wasm_slab = run_artifact_slab(artifact);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let mut vm = Vm::new(compiled.clone()).expect("vm creation");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();

    let mut checked = 0usize;
    let mut max_diff = 0.0f64;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks.min(vm_results.step_count) {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            let diff = (vm_val - wasm_val).abs();
            if diff > max_diff && vm_val.is_finite() && wasm_val.is_finite() {
                max_diff = diff;
            }
        }
        checked += 1;
    }
    (checked, max_diff)
}

// ── Per-model measurement ─────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct ModelResult {
    label: &'static str,
    n_slots: usize,
    n_chunks: usize,
    opcodes: usize,
    blob_bytes: usize,
    vars_checked: usize,
    max_diff: f64,
    // time (median ms)
    frontend: Stat,
    vm_build: Stat,
    wasm_codegen: Stat,
    wasm_validate: Stat,
    wasm_instantiate: Stat,
    vm_eval: Stat,
    wasm_eval: Stat,
    // memory (one measured pass)
    frontend_mem: Mem,
    vm_build_mem: Mem,
    vm_eval_mem: Mem,
    wasm_codegen_mem: Mem,
    wasm_instantiate_mem: Mem,
    wasm_eval_mem: Mem,
}

fn measure_model(
    spec: &ModelSpec,
    min_iters: usize,
    max_iters: usize,
    budget_s: f64,
) -> ModelResult {
    println!("\n========== {} ({}) ==========", spec.label, spec.rel_path);
    let datamodel = load_datamodel(spec);
    let n_models = datamodel.models.len();
    let n_vars: usize = datamodel.models.iter().map(|m| m.variables.len()).sum();
    println!(
        "  datamodel: {n_models} model(s), {n_vars} variables, {} dimension(s)",
        datamodel.dimensions.len()
    );

    // One real front-end compile to drive the backends; the timing loop rebuilds
    // a fresh db each iteration (a fair, cache-cold compile).
    let compiled = front_end_compile(&datamodel);
    let n_slots = compiled.n_slots();
    let prof = compiled.bytecode_profile();
    let opcodes = prof.total_opcodes;

    let artifact = compile_simulation(&compiled).expect("wasm codegen");
    let blob_bytes = artifact.wasm.len();
    let n_chunks = artifact.layout.n_chunks;
    println!(
        "  n_slots {n_slots}, n_chunks {n_chunks}, {opcodes} opcodes, blob {:.1} KiB",
        blob_bytes as f64 / 1024.0
    );

    let (vars_checked, max_diff) = cross_check(&compiled, &artifact);
    println!("  cross-check: {vars_checked} variables match VM, max abs diff {max_diff:.2e}");

    // ── Time pass (counting allocator stays off) ──
    println!("  --- timing (median of N) ---");

    let frontend = bench(min_iters, max_iters, budget_s, || {
        let t0 = Instant::now();
        let c = front_end_compile(&datamodel);
        let e = ms_since(t0);
        black_box(c);
        e
    });

    // The CompiledSimulation clone is a benchmark artifact (the wasm side reuses
    // `compiled`); in production the owned value goes straight into Vm::new, so
    // the clone is done untimed in setup.
    let vm_build = bench(min_iters, max_iters, budget_s, || {
        let c = compiled.clone();
        let t0 = Instant::now();
        let vm = Vm::new(c).expect("vm build");
        let e = ms_since(t0);
        black_box(vm);
        e
    });

    let wasm_codegen = bench(min_iters, max_iters, budget_s, || {
        let t0 = Instant::now();
        let a = compile_simulation(&compiled).expect("wasm codegen");
        let e = ms_since(t0);
        black_box(a);
        e
    });

    let wasm_validate = bench(min_iters, max_iters, budget_s, || {
        let t0 = Instant::now();
        let info = validate(&artifact.wasm).expect("validate");
        let e = ms_since(t0);
        black_box(&info);
        e
    });

    let wasm_instantiate = bench(min_iters, max_iters, budget_s, || {
        let info = validate(&artifact.wasm).expect("validate"); // untimed setup
        let t0 = Instant::now();
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let e = ms_since(t0);
        black_box(inst);
        e
    });

    // Eval / re-run: VM = reset + run_to_end on a persistent Vm; wasm = run() on
    // a persistent instance. Both exclude build/instantiate -- the interactive
    // "re-run after a parameter change" cost.
    let vm_eval = {
        let mut vm = Vm::new(compiled.clone()).expect("vm build");
        bench(min_iters, max_iters, budget_s, || {
            vm.reset();
            let t0 = Instant::now();
            vm.run_to_end().expect("vm run");
            ms_since(t0)
        })
    };

    let wasm_eval = {
        let info = validate(&artifact.wasm).expect("validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        bench(min_iters, max_iters, budget_s, || {
            // Re-fetch the func handle (cheap) untimed, mirroring the existing
            // repeated-run test helper, then time just the invocation.
            let run = store
                .instance_export(inst, "run")
                .unwrap()
                .as_func()
                .unwrap();
            let t0 = Instant::now();
            store
                .invoke_simple_typed::<(), ()>(run, ())
                .expect("run wasm");
            ms_since(t0)
        })
    };

    print_time_line("front-end compile (shared)", frontend);
    print_time_line("VM build (Vm::new)", vm_build);
    print_time_line("wasm codegen (compile_simulation)", wasm_codegen);
    print_time_line("wasm validate", wasm_validate);
    print_time_line("wasm instantiate", wasm_instantiate);
    print_time_line("VM eval (reset+run_to_end)", vm_eval);
    print_time_line("wasm eval (run)", wasm_eval);

    // ── Memory pass (counting allocator on for its full duration) ──
    println!("  --- memory (one pass, counting allocator) ---");
    ALLOC_CALLS.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_BYTES.store(0, Ordering::Relaxed);
    COUNTING_ON.store(true, Ordering::Relaxed);

    let frontend_mem = mem_phase("front-end compile (shared)", || {
        black_box(front_end_compile(&datamodel));
    });

    let compiled_mem = front_end_compile(&datamodel); // keep one for the backend phases
    let vm_build_mem = mem_phase("VM build (Vm::new)", || {
        black_box(Vm::new(compiled_mem.clone()).expect("vm build"));
    });

    let mut vm_for_eval = Vm::new(compiled_mem.clone()).expect("vm build");
    let vm_eval_mem = mem_phase("VM eval (reset+run_to_end)", || {
        vm_for_eval.reset();
        vm_for_eval.run_to_end().expect("vm run");
    });
    drop(vm_for_eval);

    let artifact_mem = compile_simulation(&compiled_mem).expect("wasm codegen");
    let wasm_codegen_mem = mem_phase("wasm codegen (compile_simulation)", || {
        black_box(compile_simulation(&compiled_mem).expect("wasm codegen"));
    });

    // Keep `info` + `store` alive across instantiate and eval so the instance's
    // borrow of the validation info outlives both measured phases.
    reset_peak();
    let before_inst = snap();
    let info = validate(&artifact_mem.wasm).expect("validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let after_inst = snap();
    let wasm_instantiate_mem =
        mem_between(before_inst, after_inst, PEAK_BYTES.load(Ordering::Relaxed));
    print_mem_line("wasm validate+instantiate", wasm_instantiate_mem);

    reset_peak();
    let before_run = snap();
    let run = store
        .instance_export(inst, "run")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run wasm");
    let after_run = snap();
    let wasm_eval_mem = mem_between(before_run, after_run, PEAK_BYTES.load(Ordering::Relaxed));
    print_mem_line("wasm eval (run)", wasm_eval_mem);

    drop(store);
    drop(info);
    COUNTING_ON.store(false, Ordering::Relaxed);

    ModelResult {
        label: spec.label,
        n_slots,
        n_chunks,
        opcodes,
        blob_bytes,
        vars_checked,
        max_diff,
        frontend,
        vm_build,
        wasm_codegen,
        wasm_validate,
        wasm_instantiate,
        vm_eval,
        wasm_eval,
        frontend_mem,
        vm_build_mem,
        vm_eval_mem,
        wasm_codegen_mem,
        wasm_instantiate_mem,
        wasm_eval_mem,
    }
}

fn mem_phase(label: &str, f: impl FnOnce()) -> Mem {
    reset_peak();
    let before = snap();
    f();
    let after = snap();
    let m = mem_between(before, after, PEAK_BYTES.load(Ordering::Relaxed));
    print_mem_line(label, m);
    m
}

fn print_time_line(label: &str, s: Stat) {
    println!(
        "    {label:<34} {:>12} ms (median, n={}, min {})",
        fmt_ms(s.median_ms),
        s.iters,
        fmt_ms(s.min_ms),
    );
}

fn print_mem_line(label: &str, m: Mem) {
    println!(
        "    {label:<34} allocs {:>10} | alloc'd {:>8.2} MiB | retained {:>+7.2} MiB | peak {:>7.2} MiB",
        m.calls, m.alloc_mib, m.retained_mib, m.peak_mib
    );
}

// ── Summary tables (markdown) ─────────────────────────────────────────────────

fn ratio(wasm: f64, vm: f64) -> String {
    if vm > 0.0 && wasm.is_finite() {
        format!("{:.2}x", wasm / vm)
    } else {
        "-".to_string()
    }
}

fn print_summary(results: &[ModelResult]) {
    println!("\n\n############ SUMMARY (markdown) ############\n");

    println!("### Model shape\n");
    println!("| model | vars (slots) | saved steps | opcodes | wasm blob | wasm vs VM max diff |");
    println!("|---|--:|--:|--:|--:|--:|");
    for r in results {
        println!(
            "| {} | {} | {} | {} | {:.1} KiB | {:.1e} ({} vars) |",
            r.label,
            r.n_slots,
            r.n_chunks,
            r.opcodes,
            r.blob_bytes as f64 / 1024.0,
            r.max_diff,
            r.vars_checked,
        );
    }

    println!("\n### Time -- backend build (ms, median)\n");
    println!(
        "| model | VM `Vm::new` | wasm codegen | wasm validate | wasm instantiate | wasm build total | wasm/VM |"
    );
    println!("|---|--:|--:|--:|--:|--:|--:|");
    for r in results {
        let wasm_total =
            r.wasm_codegen.median_ms + r.wasm_validate.median_ms + r.wasm_instantiate.median_ms;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.label,
            fmt_ms(r.vm_build.median_ms),
            fmt_ms(r.wasm_codegen.median_ms),
            fmt_ms(r.wasm_validate.median_ms),
            fmt_ms(r.wasm_instantiate.median_ms),
            fmt_ms(wasm_total),
            ratio(wasm_total, r.vm_build.median_ms),
        );
    }

    println!("\n### Time -- eval / re-run (ms, median)\n");
    println!("| model | VM reset+run | wasm run | wasm/VM | front-end compile (shared) |");
    println!("|---|--:|--:|--:|--:|");
    for r in results {
        println!(
            "| {} | {} | {} | {} | {} |",
            r.label,
            fmt_ms(r.vm_eval.median_ms),
            fmt_ms(r.wasm_eval.median_ms),
            ratio(r.wasm_eval.median_ms, r.vm_eval.median_ms),
            fmt_ms(r.frontend.median_ms),
        );
    }

    println!("\n### Memory -- peak live bytes per phase (MiB)\n");
    println!(
        "| model | front-end | VM build | VM eval | wasm codegen | wasm validate+inst | wasm eval |"
    );
    println!("|---|--:|--:|--:|--:|--:|--:|");
    for r in results {
        println!(
            "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
            r.label,
            r.frontend_mem.peak_mib,
            r.vm_build_mem.peak_mib,
            r.vm_eval_mem.peak_mib,
            r.wasm_codegen_mem.peak_mib,
            r.wasm_instantiate_mem.peak_mib,
            r.wasm_eval_mem.peak_mib,
        );
    }

    println!("\n### Memory -- allocations per phase (count)\n");
    println!(
        "| model | front-end | VM build | VM eval | wasm codegen | wasm validate+inst | wasm eval |"
    );
    println!("|---|--:|--:|--:|--:|--:|--:|");
    for r in results {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.label,
            r.frontend_mem.calls,
            r.vm_build_mem.calls,
            r.vm_eval_mem.calls,
            r.wasm_codegen_mem.calls,
            r.wasm_instantiate_mem.calls,
            r.wasm_eval_mem.calls,
        );
    }
}

fn main() {
    let selected: Vec<&'static str> = match std::env::var("BENCH_MODELS") {
        Ok(v) => v
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(leak)
            .collect(),
        Err(_) => MODELS.iter().map(|m| m.key).collect(),
    };
    let budget_s: f64 = std::env::var("BENCH_TIME_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.5);
    let min_iters = 1;
    let max_iters = 100;

    println!("backend_bench: bytecode VM vs wasm-under-DLR-FT-interpreter");
    println!("per-phase budget {budget_s}s (min {min_iters}, max {max_iters} iters)");

    let mut results = Vec::new();
    for spec in MODELS {
        if selected.contains(&spec.key) {
            results.push(measure_model(spec, min_iters, max_iters, budget_s));
        }
    }

    print_summary(&results);
}

/// Leak a runtime model-key string so it can live in `&'static str` comparisons
/// against the compile-time `MODELS` keys (one-shot CLI parse; never freed).
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
