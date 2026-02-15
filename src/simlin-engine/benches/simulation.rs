// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use simlin_engine::common::Ident;
use simlin_engine::test_common::TestProject;
use simlin_engine::{CompiledSimulation, Project as CompiledProject, Simulation, Vm};

fn build_population_project(stop: f64) -> TestProject {
    TestProject::new("bench_pop")
        .with_sim_time(0.0, stop, 1.0)
        .aux("birth_rate", "0.1", None)
        .aux("lifespan", "80", None)
        .aux("initial_pop", "1000 * birth_rate", None)
        .stock("population", "initial_pop", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population / lifespan", None)
}

fn compile_population(stop: f64) -> CompiledSimulation {
    let tp = build_population_project(stop);
    let datamodel = tp.build_datamodel();
    let project = Arc::new(CompiledProject::from(datamodel));
    let sim = Simulation::new(&project, "main").unwrap();
    sim.compile().unwrap()
}

fn bench_compile(c: &mut Criterion) {
    let tp = build_population_project(1000.0);
    let datamodel = tp.build_datamodel();
    let project = Arc::new(CompiledProject::from(datamodel));

    c.bench_function("compile", |b| {
        b.iter(|| {
            let sim = Simulation::new(&project, "main").unwrap();
            sim.compile().unwrap()
        })
    });
}

fn bench_vm_create(c: &mut Criterion) {
    let compiled = compile_population(1000.0);

    c.bench_function("vm_create", |b| {
        b.iter(|| Vm::new(compiled.clone()).unwrap())
    });
}

fn bench_run_simulation(c: &mut Criterion) {
    let mut group = c.benchmark_group("run_simulation");
    group.measurement_time(Duration::from_secs(10));

    for &steps in &[1_000, 10_000, 100_000, 1_000_000] {
        let compiled = compile_population(steps as f64);

        group.bench_with_input(
            BenchmarkId::from_parameter(steps),
            &compiled,
            |b, compiled| {
                b.iter(|| {
                    let mut vm = Vm::new(compiled.clone()).unwrap();
                    vm.run_to_end().unwrap();
                })
            },
        );
    }
    group.finish();
}

fn bench_slider_interaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("slider_interaction");
    group.measurement_time(Duration::from_secs(10));

    for &steps in &[1_000, 10_000, 100_000, 1_000_000] {
        let compiled = compile_population(steps as f64);
        let ident = Ident::new("birth_rate");

        group.bench_with_input(
            BenchmarkId::from_parameter(steps),
            &(compiled, ident),
            |b, (compiled, ident)| {
                b.iter_batched(
                    || {
                        let mut vm = Vm::new(compiled.clone()).unwrap();
                        vm.set_value(ident, 0.2).unwrap();
                        vm
                    },
                    |mut vm| {
                        vm.reset();
                        vm.run_to_end().unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }
    group.finish();
}

fn bench_full_pipeline(c: &mut Criterion) {
    let tp = build_population_project(1000.0);
    let datamodel = tp.build_datamodel();

    c.bench_function("full_pipeline", |b| {
        b.iter(|| {
            let project = Arc::new(CompiledProject::from(datamodel.clone()));
            let sim = Simulation::new(&project, "main").unwrap();
            let compiled = sim.compile().unwrap();
            let mut vm = Vm::new(compiled).unwrap();
            vm.run_to_end().unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_compile,
    bench_vm_create,
    bench_run_simulation,
    bench_slider_interaction,
    bench_full_pipeline,
);
criterion_main!(benches);
