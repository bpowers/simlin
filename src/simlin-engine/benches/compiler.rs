// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compiler benchmarks using real-world SD models (WRLD3, C-LEARN).
//!
//! These benchmarks measure the performance of each compilation stage on large,
//! production-grade models to establish baselines for optimization work.
//!
//! ## Benchmark groups
//!
//! - `parse_mdl` — MDL text → `datamodel::Project` (lexing + parsing + conversion)
//! - `project_build` — `datamodel::Project` → engine `Project` (unit inference, dependency resolution)
//! - `bytecode_compile` — engine `Project` → `CompiledSimulation` (bytecode generation)
//! - `full_pipeline` — MDL text → `CompiledSimulation` (all stages end-to-end)
//!
//! ## Notes
//!
//! Not all models support bytecode compilation (some use unsupported builtins).
//! The `bytecode_compile` and `full_pipeline_models` groups skip models that
//! return `NotSimulatable`.
//!
//! ## Profiling with external tools
//!
//! See `doc/dev/benchmarks.md` for instructions on using valgrind/callgrind,
//! perf, and gperftools/heaptrack to analyze allocations and CPU time.

use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use simlin_engine::Project as CompiledProject;
use simlin_engine::{Simulation, open_vensim};

/// Model metadata for benchmark parameterization.
struct ModelFixture {
    name: &'static str,
    path: &'static str,
}

static MODELS: &[ModelFixture] = &[
    ModelFixture {
        name: "wrld3",
        path: "../../test/metasd/WRLD3-03/wrld3-03.mdl",
    },
    ModelFixture {
        name: "clearn",
        path: "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
    },
];

/// Load model contents at benchmark setup time.  Panics on missing files so
/// CI catches missing test data immediately.
fn load_model(fixture: &ModelFixture) -> String {
    std::fs::read_to_string(fixture.path).unwrap_or_else(|e| {
        panic!(
            "failed to read model file '{}': {e}\n\
             (run from src/simlin-engine/ or the repo root)",
            fixture.path
        )
    })
}

/// Check whether a compiled project can be compiled to bytecode.
/// Some models use builtins or features that prevent simulation.
fn is_simulatable(project: &CompiledProject) -> bool {
    Simulation::new(project, "main")
        .and_then(|sim| sim.compile())
        .is_ok()
}

/// Benchmark: MDL text → datamodel::Project.
///
/// Measures the combined cost of lexing, parsing, and MDL-to-datamodel
/// conversion.
fn bench_parse_mdl(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_mdl");
    group.measurement_time(Duration::from_secs(10));

    for fixture in MODELS {
        let contents = load_model(fixture);
        group.bench_with_input(
            BenchmarkId::from_parameter(fixture.name),
            &contents,
            |b, contents| {
                b.iter(|| black_box(open_vensim(contents).unwrap()));
            },
        );
    }

    group.finish();
}

/// Benchmark: datamodel::Project → engine Project.
///
/// Measures unit inference, dependency resolution, topological sorting,
/// and stdlib loading.  The parse step is excluded.
fn bench_project_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("project_build");
    group.measurement_time(Duration::from_secs(10));

    for fixture in MODELS {
        let contents = load_model(fixture);
        let datamodel = open_vensim(&contents).unwrap();

        group.bench_with_input(
            BenchmarkId::from_parameter(fixture.name),
            &datamodel,
            |b, datamodel| {
                b.iter_batched(
                    || datamodel.clone(),
                    |dm| black_box(CompiledProject::from(dm)),
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark: engine Project → CompiledSimulation (bytecode).
///
/// Measures bytecode compilation only: Simulation::new() + sim.compile().
/// Parse and project-build costs are excluded.  Models that cannot be
/// compiled to bytecode (e.g. due to unsupported builtins) are skipped.
fn bench_bytecode_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytecode_compile");
    group.measurement_time(Duration::from_secs(10));

    for fixture in MODELS {
        let contents = load_model(fixture);
        let datamodel = open_vensim(&contents).unwrap();
        let project = Arc::new(CompiledProject::from(datamodel));

        if !is_simulatable(&project) {
            eprintln!(
                "skipping bytecode_compile/{}: model is not simulatable",
                fixture.name
            );
            continue;
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(fixture.name),
            &project,
            |b, project| {
                b.iter(|| {
                    let sim = Simulation::new(project, "main").unwrap();
                    black_box(sim.compile().unwrap())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark: MDL text → CompiledSimulation (full pipeline).
///
/// Measures the entire compilation pipeline end-to-end, including parsing,
/// project building, and bytecode compilation.  Useful for seeing total
/// wall-clock cost and for comparing against the sum of individual stages.
/// Models that cannot be compiled to bytecode are skipped.
fn bench_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_pipeline_models");
    group.measurement_time(Duration::from_secs(15));

    for fixture in MODELS {
        let contents = load_model(fixture);

        // Pre-check simulatability so we don't panic inside the benchmark loop.
        let datamodel = open_vensim(&contents).unwrap();
        let project = CompiledProject::from(datamodel);
        if !is_simulatable(&project) {
            eprintln!(
                "skipping full_pipeline_models/{}: model is not simulatable",
                fixture.name
            );
            continue;
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(fixture.name),
            &contents,
            |b, contents| {
                b.iter(|| {
                    let datamodel = open_vensim(contents).unwrap();
                    let project = Arc::new(CompiledProject::from(datamodel));
                    let sim = Simulation::new(&project, "main").unwrap();
                    black_box(sim.compile().unwrap())
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_parse_mdl,
    bench_project_build,
    bench_bytecode_compile,
    bench_full_pipeline,
);
criterion_main!(benches);
