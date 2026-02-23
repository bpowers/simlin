// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compiler benchmarks using real-world SD models (WRLD3, C-LEARN) and
//! salsa-based incremental compilation.
//!
//! ## Benchmark groups
//!
//! - `parse_mdl` -- MDL text -> `datamodel::Project` (lexing + parsing + conversion)
//! - `project_build` -- `datamodel::Project` -> engine `Project` (unit inference, dependency resolution)
//! - `bytecode_compile` -- engine `Project` -> `CompiledSimulation` (bytecode generation)
//! - `full_pipeline` -- MDL text -> `CompiledSimulation` (all stages end-to-end)
//! - `salsa_incremental` -- compares monolithic `compile_project` vs incremental
//!   `compile_project_incremental` on synthetic chain models after a single-variable
//!   equation edit or variable add/remove
//!
//! ## Notes
//!
//! Not all models support bytecode compilation (some use unsupported builtins).
//! The `bytecode_compile` and `full_pipeline` groups skip models that
//! return `NotSimulatable`.
//!
//! ## Profiling with external tools
//!
//! See `doc/dev/benchmarks.md` for instructions on using valgrind/callgrind,
//! perf, and gperftools/heaptrack to analyze allocations and CPU time.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use simlin_engine::Project as CompiledProject;
use simlin_engine::datamodel::{
    self, Aux, Compat, Dt, Equation, SimMethod, SimSpecs, Variable, Visibility,
};
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::{Simulation, compile_project, open_vensim};

/// Model metadata for benchmark parameterization.
struct ModelFixture {
    name: &'static str,
    /// Path relative to the workspace root (i.e. the repo's `test/` directory).
    rel_path: &'static str,
}

static MODELS: &[ModelFixture] = &[
    ModelFixture {
        name: "wrld3",
        rel_path: "test/metasd/WRLD3-03/wrld3-03.mdl",
    },
    ModelFixture {
        name: "clearn",
        rel_path: "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
    },
];

/// Resolve a fixture path to an absolute path using CARGO_MANIFEST_DIR.
fn fixture_path(fixture: &ModelFixture) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(fixture.rel_path)
}

/// Load model contents at benchmark setup time.  Panics on missing files so
/// CI catches missing test data immediately.
fn load_model(fixture: &ModelFixture) -> String {
    let path = fixture_path(fixture);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read model file '{}': {e}", path.display()))
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
    let mut group = c.benchmark_group("full_pipeline");
    group.measurement_time(Duration::from_secs(15));

    for fixture in MODELS {
        let contents = load_model(fixture);

        // Pre-check simulatability so we don't panic inside the benchmark loop.
        let datamodel = open_vensim(&contents).unwrap();
        let project = CompiledProject::from(datamodel);
        if !is_simulatable(&project) {
            eprintln!(
                "skipping full_pipeline/{}: model is not simulatable",
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

// ── Incremental compilation benchmarks ──────────────────────────────────

/// Build a chain model with `n` variables: v1 = 1, v2 = v1 + 1, ..., vN = v(N-1) + 1.
fn build_chain_model(n: usize) -> datamodel::Project {
    let mut variables = Vec::with_capacity(n);
    for i in 1..=n {
        let equation = if i == 1 {
            "1".to_string()
        } else {
            format!("v{} + 1", i - 1)
        };
        variables.push(Variable::Aux(Aux {
            ident: format!("v{i}"),
            equation: Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Private,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
    }

    datamodel::Project {
        name: "chain_bench".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: Default::default(),
        ai_information: None,
    }
}

/// AC5.1: Equation edit benchmark.
///
/// Compares full recompilation against incremental recompilation after
/// changing a single variable's equation (v50: "v49 + 1" -> "v49 + 2").
fn bench_incremental_equation_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("salsa_incremental/equation_edit");
    group.measurement_time(Duration::from_secs(10));

    let n = 100;
    let original = build_chain_model(n);

    // Mutated version: change v50's equation
    let mut mutated = original.clone();
    for var in &mut mutated.models[0].variables {
        if let Variable::Aux(aux) = var
            && aux.ident == "v50"
        {
            aux.equation = Equation::Scalar("v49 + 2".to_string());
        }
    }

    // -- Monolithic baseline: full compile from scratch on the mutated model
    group.bench_function(BenchmarkId::new("monolithic", n), |b| {
        b.iter(|| {
            let project = CompiledProject::from(mutated.clone());
            black_box(compile_project(&project, "main").unwrap())
        });
    });

    // -- Incremental: sync original, then sync mutation and recompile
    group.bench_function(BenchmarkId::new("incremental", n), |b| {
        // One-time setup: create the db and do the initial sync + compile
        // so that salsa has a warm cache for the original model.
        let mut db = SimlinDb::default();
        let state = sync_from_datamodel_incremental(&mut db, &original, None);
        let sync = state.to_sync_result();
        compile_project_incremental(&db, sync.project, "main").unwrap();

        b.iter(|| {
            // Incrementally sync the mutated project and recompile.
            // The db retains cached results from the previous revision;
            // only v50 and its dependents should be re-evaluated.
            let state2 = sync_from_datamodel_incremental(&mut db, &mutated, Some(&state));
            let sync2 = state2.to_sync_result();
            black_box(compile_project_incremental(&db, sync2.project, "main").unwrap())
        });
    });

    group.finish();
}

/// AC5.2: Variable add/remove benchmark.
///
/// Compares full recompilation against incremental recompilation after
/// adding a new variable (v101 = v100 + 1) or removing the last variable.
fn bench_incremental_add_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("salsa_incremental/add_remove");
    group.measurement_time(Duration::from_secs(10));

    let n = 100;
    let original = build_chain_model(n);

    // Extended version: add v101 = v100 + 1
    let extended = build_chain_model(n + 1);

    // Reduced version: remove v100, so v99 is the last
    let reduced = build_chain_model(n - 1);

    // -- Monolithic baseline: full compile of the extended model
    group.bench_function(BenchmarkId::new("monolithic_add", n), |b| {
        b.iter(|| {
            let project = CompiledProject::from(extended.clone());
            black_box(compile_project(&project, "main").unwrap())
        });
    });

    // -- Incremental: add v101
    group.bench_function(BenchmarkId::new("incremental_add", n), |b| {
        let mut db = SimlinDb::default();
        let state = sync_from_datamodel_incremental(&mut db, &original, None);
        let sync = state.to_sync_result();
        compile_project_incremental(&db, sync.project, "main").unwrap();

        b.iter(|| {
            let state2 = sync_from_datamodel_incremental(&mut db, &extended, Some(&state));
            let sync2 = state2.to_sync_result();
            black_box(compile_project_incremental(&db, sync2.project, "main").unwrap())
        });
    });

    // -- Monolithic baseline: full compile of the reduced model
    group.bench_function(BenchmarkId::new("monolithic_remove", n), |b| {
        b.iter(|| {
            let project = CompiledProject::from(reduced.clone());
            black_box(compile_project(&project, "main").unwrap())
        });
    });

    // -- Incremental: remove v100
    group.bench_function(BenchmarkId::new("incremental_remove", n), |b| {
        let mut db = SimlinDb::default();
        let state = sync_from_datamodel_incremental(&mut db, &original, None);
        let sync = state.to_sync_result();
        compile_project_incremental(&db, sync.project, "main").unwrap();

        b.iter(|| {
            let state2 = sync_from_datamodel_incremental(&mut db, &reduced, Some(&state));
            let sync2 = state2.to_sync_result();
            black_box(compile_project_incremental(&db, sync2.project, "main").unwrap())
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parse_mdl,
    bench_project_build,
    bench_bytecode_compile,
    bench_full_pipeline,
    bench_incremental_equation_edit,
    bench_incremental_add_remove,
);
criterion_main!(benches);
