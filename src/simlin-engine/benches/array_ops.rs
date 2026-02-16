// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Benchmarks for array operations in the simulation engine.
//!
//! These benchmarks measure the performance of various array operations,
//! particularly focusing on view management overhead in iteration contexts.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use simlin_engine::Project as CompiledProject;
use simlin_engine::Simulation;
use simlin_engine::Vm;
use simlin_engine::datamodel::Project;
use simlin_engine::datamodel::{
    Aux, Dimension, Dt, Equation, SimMethod, SimSpecs, Variable, Visibility,
};
use std::sync::Arc;

/// Create a project with a single large 1D array and a sum reduction
fn create_sum_project(array_size: u32) -> Project {
    let dim_name = "Idx";
    Project {
        name: format!("sum_benchmark_{array_size}"),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![Dimension::indexed(dim_name.to_string(), array_size)],
        units: vec![],
        models: vec![simlin_engine::datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Aux(Aux {
                    ident: "arr".to_string(),
                    equation: Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        "1.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                Variable::Aux(Aux {
                    ident: "total".to_string(),
                    equation: Equation::Scalar("SUM(arr[*])".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: Default::default(),
        ai_information: None,
    }
}

/// Create a project with two arrays and element-wise addition: a[*] + b[*]
fn create_elementwise_add_project(array_size: u32) -> Project {
    let dim_name = "Idx";
    Project {
        name: format!("elementwise_add_{array_size}"),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![Dimension::indexed(dim_name.to_string(), array_size)],
        units: vec![],
        models: vec![simlin_engine::datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Aux(Aux {
                    ident: "a".to_string(),
                    equation: Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        "1.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                Variable::Aux(Aux {
                    ident: "b".to_string(),
                    equation: Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        "2.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                Variable::Aux(Aux {
                    ident: "result".to_string(),
                    equation: Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        "a[*] + b[*]".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: Default::default(),
        ai_information: None,
    }
}

/// Create a project with multiple references to the same array: a[*] + a[*] + a[*]
fn create_same_array_multi_ref_project(array_size: u32, num_refs: usize) -> Project {
    let dim_name = "Idx";

    // Build equation like "a[*] + a[*] + a[*]"
    let equation = (0..num_refs)
        .map(|_| "a[*]")
        .collect::<Vec<_>>()
        .join(" + ");

    Project {
        name: format!("same_array_multi_ref_{array_size}_{num_refs}"),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![Dimension::indexed(dim_name.to_string(), array_size)],
        units: vec![],
        models: vec![simlin_engine::datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Aux(Aux {
                    ident: "a".to_string(),
                    equation: Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        "1.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                Variable::Aux(Aux {
                    ident: "result".to_string(),
                    equation: Equation::ApplyToAll(vec![dim_name.to_string()], equation, None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: Default::default(),
        ai_information: None,
    }
}

/// Create a project with 2D arrays for broadcasting tests
fn create_broadcast_project(dim1_size: u32, dim2_size: u32) -> Project {
    Project {
        name: format!("broadcast_{}x{}", dim1_size, dim2_size),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![
            Dimension::indexed("DimA".to_string(), dim1_size),
            Dimension::indexed("DimB".to_string(), dim2_size),
        ],
        units: vec![],
        models: vec![simlin_engine::datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // 1D array on DimA
                Variable::Aux(Aux {
                    ident: "vec_a".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["DimA".to_string()],
                        "1.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                // 1D array on DimB
                Variable::Aux(Aux {
                    ident: "vec_b".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["DimB".to_string()],
                        "2.0".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
                // 2D result from broadcasting
                Variable::Aux(Aux {
                    ident: "matrix".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["DimA".to_string(), "DimB".to_string()],
                        "vec_a[*] + vec_b[*]".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: Default::default(),
        ai_information: None,
    }
}

/// Compile a project and return the simulation
fn compile_project(project: Project) -> Result<Simulation, String> {
    let compiled = Arc::new(CompiledProject::from(project));

    // Check for errors
    if !compiled.errors.is_empty() {
        return Err(format!("Project errors: {:?}", compiled.errors));
    }

    Simulation::new(&compiled, "main").map_err(|e| format!("Failed to create simulation: {e:?}"))
}

/// Benchmark array sum reduction
fn bench_array_sum(c: &mut Criterion) {
    let mut group = c.benchmark_group("array_sum");

    for size in [100, 500, 1000, 5000] {
        let project = create_sum_project(size);
        let sim = compile_project(project).expect("Should compile");
        let compiled_bytecode = sim.compile().expect("Should compile to bytecode");

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &compiled_bytecode,
            |b, bytecode| {
                b.iter(|| {
                    let mut vm = Vm::new(bytecode.clone()).expect("VM creation");
                    vm.run_to_end().expect("VM run");
                    black_box(vm.into_results())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark element-wise addition: a[*] + b[*]
fn bench_elementwise_add(c: &mut Criterion) {
    let mut group = c.benchmark_group("elementwise_add");

    for size in [100, 500, 1000, 5000] {
        let project = create_elementwise_add_project(size);
        let sim = compile_project(project).expect("Should compile");
        let compiled_bytecode = sim.compile().expect("Should compile to bytecode");

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &compiled_bytecode,
            |b, bytecode| {
                b.iter(|| {
                    let mut vm = Vm::new(bytecode.clone()).expect("VM creation");
                    vm.run_to_end().expect("VM run");
                    black_box(vm.into_results())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark multiple references to the same array: a[*] + a[*] + a[*]
fn bench_same_array_refs(c: &mut Criterion) {
    let mut group = c.benchmark_group("same_array_refs");

    // Test with 1000 elements and varying number of references
    let size = 1000;
    for num_refs in [2, 3, 4, 5] {
        let project = create_same_array_multi_ref_project(size, num_refs);
        let sim = compile_project(project).expect("Should compile");
        let compiled_bytecode = sim.compile().expect("Should compile to bytecode");

        group.bench_with_input(
            BenchmarkId::new("refs", num_refs),
            &compiled_bytecode,
            |b, bytecode| {
                b.iter(|| {
                    let mut vm = Vm::new(bytecode.clone()).expect("VM creation");
                    vm.run_to_end().expect("VM run");
                    black_box(vm.into_results())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark broadcasting: vec_a[DimA] + vec_b[DimB] -> matrix[DimA, DimB]
fn bench_broadcast(c: &mut Criterion) {
    let mut group = c.benchmark_group("broadcast");

    for (dim1, dim2) in [(10, 100), (50, 50), (100, 100)] {
        let project = create_broadcast_project(dim1, dim2);
        let sim = compile_project(project).expect("Should compile");
        let compiled_bytecode = sim.compile().expect("Should compile to bytecode");

        group.bench_with_input(
            BenchmarkId::new("dims", format!("{}x{}", dim1, dim2)),
            &compiled_bytecode,
            |b, bytecode| {
                b.iter(|| {
                    let mut vm = Vm::new(bytecode.clone()).expect("VM creation");
                    vm.run_to_end().expect("VM run");
                    black_box(vm.into_results())
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_array_sum,
    bench_elementwise_add,
    bench_same_array_refs,
    bench_broadcast,
);

criterion_main!(benches);
