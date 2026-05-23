# Benchmarks

## Running benchmarks

All benchmarks use [Criterion.rs](https://bheisler.github.io/criterion.rs/book/) and live in `src/simlin-engine/benches/`.

```bash
# Run all benchmarks
cargo bench -p simlin-engine

# Run a specific benchmark suite
cargo bench -p simlin-engine --bench compiler
cargo bench -p simlin-engine --bench simulation
cargo bench -p simlin-engine --bench array_ops

# Run a specific benchmark group within a suite
cargo bench -p simlin-engine --bench compiler -- parse_mdl
cargo bench -p simlin-engine --bench compiler -- bytecode_compile/clearn
```

Criterion saves results in `target/criterion/` and generates HTML reports in `target/criterion/<group>/report/index.html`.

## Benchmark suites

| Suite        | File                    | What it measures                                                  |
| ------------ | ----------------------- | ----------------------------------------------------------------- |
| `compiler`   | `benches/compiler.rs`   | End-to-end compiler pipeline on real models (WRLD3, C-LEARN)      |
| `simulation` | `benches/simulation.rs` | VM execution, slider interaction, compilation of synthetic models |
| `array_ops`  | `benches/array_ops.rs`  | Array sum, element-wise add, broadcasting, multi-ref              |

### compiler benchmarks

The `compiler` suite measures each stage of the compilation pipeline independently using large, real-world models checked in under `test/`:

- **`parse_mdl`** — MDL text to `datamodel::Project` (lexing, parsing, conversion)
- **`project_build`** — `datamodel::Project` to engine `Project` (unit inference, dependency resolution, topological sort)
- **`bytecode_compile`** — engine `Project` to `CompiledSimulation` (bytecode generation)
- **`full_pipeline`** — all stages end-to-end

Models used:

- `wrld3` — World3 model (151 KB, ~3,800 lines), a classic system dynamics model
- `clearn` — C-LEARN climate model (1.4 MB, ~53,000 lines), a stress test for the compiler

C-LEARN currently uses builtins that are not yet implemented in the bytecode compiler, so it is automatically skipped for `bytecode_compile` and `full_pipeline`. It still participates in `parse_mdl` and `project_build`, which are the most allocation-heavy stages.

## Node VM-vs-wasm eval benchmark

`@simlin/engine` can run a model on two backends: the libsimlin VM or a compiled WebAssembly blob. This benchmark compares their **simulation (eval) time** through the public `Model.simulate({ engine })` API, on fishbanks, WORLD3, and C-LEARN.

It is a [jest](https://jestjs.io/) test gated behind `RUN_BENCH` so it stays out of the default `pnpm test` (a full C-LEARN run on both engines exceeds the per-test time budget):

```bash
# Run all three models on both engines
RUN_BENCH=1 pnpm -C src/engine exec jest backend-bench

# Subset the models (comma-separated: fishbanks, wrld3, clearn)
RUN_BENCH=1 BENCH_MODELS=fishbanks,wrld3 pnpm -C src/engine exec jest backend-bench
```

It prints a markdown table of the warm **median** eval time per engine plus the wasm/VM ratio.

What it measures, and what it deliberately excludes:

- **Eval only.** The `Sim` for each `(model, engine)` is built once in untimed setup; for wasm that one-time cost is the blob compile and instantiate. Each measured iteration is a `reset()` (also untimed) followed by a timed `runToEnd()`. Result extraction (`getRun`/`getSeries`) is not timed.
- **Median over an explicit warmup.** A discard-only warmup runs first, then the harness collects timings adaptively (until a max iteration count or a per-model wall-clock budget) and reports the median. The pure stats/harness lives in `src/engine/tests/bench-stats.ts` and is always-on unit-tested.
- **Cross-checked before trusted.** Before timing, the benchmark runs each model on both engines and compares a representative series within the engine's tolerance, so a broken run can't masquerade as a fast one.

Absolute numbers include the async public-API overhead, so the VM/wasm ratio is the figure to compare across runs.

The Rust counterpart is `src/simlin-engine/examples/backend_bench.rs`, which uses the same eval-vs-eval methodology and median statistic against the lower-level `Vm`/wasm interfaces.

Results are reported in the PR or chat, not committed: the harness is regenerable, but checked-in numbers go stale and mislead. Do not add a results file.

## Profiling

### Build a benchmark binary for profiling

Criterion benchmark binaries are standalone executables. To build one without running it:

```bash
cargo bench -p simlin-engine --bench compiler --no-run
```

The binary will be in `target/release/deps/compiler-<hash>`. Find the exact path with:

```bash
cargo bench -p simlin-engine --bench compiler --no-run 2>&1 | grep -o 'target/[^ ]*'
```

### CPU profiling with perf (Linux)

```bash
# Record a profile (run a single benchmark to keep the profile focused)
perf record -g -- target/release/deps/compiler-* --bench parse_mdl/clearn

# View the report
perf report

# Generate a flamegraph (requires https://github.com/brendangregg/FlameGraph)
perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg
```

Alternatively, use `cargo-flamegraph`:

```bash
cargo install flamegraph
cargo flamegraph --bench compiler -- --bench parse_mdl/clearn
```

### CPU profiling with callgrind (valgrind)

Callgrind provides instruction-level profiling and call graphs. It runs the program under emulation, so it's slower but gives precise, deterministic results unaffected by system load.

```bash
# Profile a specific benchmark
valgrind --tool=callgrind --callgrind-out-file=callgrind.out \
    target/release/deps/compiler-* --bench parse_mdl/clearn

# Analyze results
callgrind_annotate callgrind.out
# Or use the graphical viewer:
kcachegrind callgrind.out
```

### Allocation profiling with DHAT (valgrind)

DHAT tracks every allocation: size, lifetime, and access patterns. Useful for finding unnecessary allocations or short-lived temporaries.

```bash
valgrind --tool=dhat \
    target/release/deps/compiler-* --bench parse_mdl/clearn

# Opens an interactive viewer in Firefox/Chrome
# The output file is dhat-out-<pid>.txt
```

View results at https://valgrind.org/docs/manual/dh-manual.html or use `dh_view.html` from the valgrind distribution.

### Allocation profiling with heaptrack

[heaptrack](https://github.com/KDE/heaptrack) is lighter-weight than DHAT and produces flamegraphs of allocation sites.

```bash
heaptrack target/release/deps/compiler-* --bench parse_mdl/clearn

# Analyze (TUI)
heaptrack_print heaptrack.compiler-*.zst

# Analyze (GUI)
heaptrack_gui heaptrack.compiler-*.zst
```

### CPU + allocation profiling with gperftools

[gperftools](https://github.com/gperftools/gperftools) provides both CPU and heap profiling via `LD_PRELOAD`.

```bash
# CPU profile
LD_PRELOAD=/usr/lib/libprofiler.so CPUPROFILE=cpu.prof \
    target/release/deps/compiler-* --bench parse_mdl/clearn

# Heap profile
LD_PRELOAD=/usr/lib/libtcmalloc.so HEAPPROFILE=heap.prof \
    target/release/deps/compiler-* --bench parse_mdl/clearn

# View (requires google-pprof or go tool pprof)
pprof --web target/release/deps/compiler-* cpu.prof
pprof --web target/release/deps/compiler-* heap.prof.0001.heap
```

### Allocation counting with the global allocator

For tracking allocation counts and bytes in CI or quick checks, Rust's global allocator can be overridden. This isn't wired up in the benchmarks by default, but you can use it in a one-off test:

```rust
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: CountingAlloc = CountingAlloc;
```

## Comparing results

Criterion automatically compares against the previous run and reports statistical significance. To save an explicit baseline for later comparison:

```bash
# Save a baseline
cargo bench -p simlin-engine --bench compiler -- --save-baseline before-change

# Run again after making changes
cargo bench -p simlin-engine --bench compiler -- --baseline before-change
```

HTML comparison reports are generated in `target/criterion/<group>/report/index.html`.

## Tips

- Use `-- <filter>` to run only matching benchmarks (e.g., `-- parse_mdl/clearn`).
- Build in release mode (`cargo bench` does this automatically) for representative profiles.
- The benchmarks configure `measurement_time` (10-15s) in code, which gives external profilers enough sustained CPU activity to collect meaningful data.
- When using valgrind tools, the 10-30x slowdown means you may want to reduce `measurement_time` in the benchmark source or filter to a single benchmark.
- For allocation analysis, compare total allocation counts before and after a change rather than absolute numbers — the counts are deterministic across runs.
