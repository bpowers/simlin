// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the systems format stdlib modules: systems_rate, systems_leak, and systems_conversion.
//!
//! Each test builds a project with a main model that instantiates a stdlib module,
//! wires input values, reads outputs, and verifies numeric results match the Python
//! `systems` package semantics.

use crate::datamodel::{self, Compat, Equation, Module, ModuleReference, Variable};
use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use crate::vm::Vm;

/// Build a `Variable::Module` for a systems stdlib module.
///
/// `ident` is the instance name in the calling model (e.g. "rate_mod").
/// `model_name` is the stdlib model (e.g. "stdlib\u{205A}systems_rate").
/// `refs` maps (src_var_in_caller, module_ident.port_name) pairs.
fn systems_module(ident: &str, model_name: &str, refs: &[(&str, &str)]) -> Variable {
    Variable::Module(Module {
        ident: ident.to_string(),
        model_name: model_name.to_string(),
        documentation: String::new(),
        units: None,
        references: refs
            .iter()
            .map(|(src, dst)| ModuleReference {
                src: src.to_string(),
                dst: dst.to_string(),
            })
            .collect(),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn x_aux(name: &str, equation: &str) -> Variable {
    Variable::Aux(datamodel::Aux {
        ident: name.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn sim_specs() -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: 0.0,
        stop: 1.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Month".to_string()),
    }
}

/// Run a project through the incremental compiler + VM and return the final-timestep
/// value of the named variable.
fn run_and_get(project: &datamodel::Project, var_name: &str) -> f64 {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("compilation failed: {e:?}"));
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e:?}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e:?}"));
    let ident = crate::common::Ident::new(var_name);
    let series = vm
        .get_series(&ident)
        .unwrap_or_else(|| panic!("variable {var_name} not found in results"));
    *series.last().expect("series should not be empty")
}

// ---------------------------------------------------------------------------
// systems_rate tests
// ---------------------------------------------------------------------------

/// Build a project that instantiates systems_rate with given inputs
/// and exposes "out_actual" and "out_remaining" as outputs.
fn rate_project(available: f64, requested: f64, dest_capacity: f64) -> datamodel::Project {
    let module_ident = "rate_mod";
    let model_name = "stdlib\u{205A}systems_rate";

    datamodel::Project {
        name: "test_systems_rate".to_string(),
        sim_specs: sim_specs(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                x_aux("available_val", &available.to_string()),
                x_aux("requested_val", &requested.to_string()),
                x_aux("dest_capacity_val", &dest_capacity.to_string()),
                systems_module(
                    module_ident,
                    model_name,
                    &[
                        ("available_val", "rate_mod.available"),
                        ("requested_val", "rate_mod.requested"),
                        ("dest_capacity_val", "rate_mod.dest_capacity"),
                    ],
                ),
                x_aux("out_actual", "rate_mod.actual"),
                x_aux("out_remaining", "rate_mod.remaining"),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    }
}

#[test]
fn test_rate_basic_transfer() {
    // available=10, requested=7, dest_capacity=INF -> actual=7, remaining=3
    let project = rate_project(10.0, 7.0, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 7.0).abs() < 1e-6,
        "expected actual=7.0, got {actual}"
    );
    assert!(
        (remaining - 3.0).abs() < 1e-6,
        "expected remaining=3.0, got {remaining}"
    );
}

#[test]
fn test_rate_limited_by_available() {
    // available=3, requested=7, dest_capacity=INF -> actual=3, remaining=0
    let project = rate_project(3.0, 7.0, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 3.0).abs() < 1e-6,
        "expected actual=3.0, got {actual}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}

#[test]
fn test_rate_limited_by_dest_capacity() {
    // available=10, requested=7, dest_capacity=5 -> actual=5, remaining=5
    let project = rate_project(10.0, 7.0, 5.0);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 5.0).abs() < 1e-6,
        "expected actual=5.0, got {actual}"
    );
    assert!(
        (remaining - 5.0).abs() < 1e-6,
        "expected remaining=5.0, got {remaining}"
    );
}

#[test]
fn test_rate_zero_available() {
    // available=0, requested=7, dest_capacity=INF -> actual=0, remaining=0
    let project = rate_project(0.0, 7.0, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 0.0).abs() < 1e-6,
        "expected actual=0.0, got {actual}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}

// ---------------------------------------------------------------------------
// systems_leak tests
// ---------------------------------------------------------------------------

/// Build a project that instantiates systems_leak with given inputs
/// and exposes "out_actual" and "out_remaining" as outputs.
fn leak_project(available: f64, rate: f64, dest_capacity: f64) -> datamodel::Project {
    let module_ident = "leak_mod";
    let model_name = "stdlib\u{205A}systems_leak";

    datamodel::Project {
        name: "test_systems_leak".to_string(),
        sim_specs: sim_specs(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                x_aux("available_val", &available.to_string()),
                x_aux("rate_val", &rate.to_string()),
                x_aux("dest_capacity_val", &dest_capacity.to_string()),
                systems_module(
                    module_ident,
                    model_name,
                    &[
                        ("available_val", "leak_mod.available"),
                        ("rate_val", "leak_mod.rate"),
                        ("dest_capacity_val", "leak_mod.dest_capacity"),
                    ],
                ),
                x_aux("out_actual", "leak_mod.actual"),
                x_aux("out_remaining", "leak_mod.remaining"),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    }
}

#[test]
fn test_leak_basic() {
    // available=100, rate=0.1, dest_capacity=INF -> actual=INT(10.0)=10, remaining=90
    let project = leak_project(100.0, 0.1, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 10.0).abs() < 1e-6,
        "expected actual=10.0, got {actual}"
    );
    assert!(
        (remaining - 90.0).abs() < 1e-6,
        "expected remaining=90.0, got {remaining}"
    );
}

#[test]
fn test_leak_with_truncation() {
    // available=15, rate=0.2, dest_capacity=INF -> actual=INT(3.0)=3, remaining=12
    let project = leak_project(15.0, 0.2, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 3.0).abs() < 1e-6,
        "expected actual=3.0, got {actual}"
    );
    assert!(
        (remaining - 12.0).abs() < 1e-6,
        "expected remaining=12.0, got {remaining}"
    );
}

#[test]
fn test_leak_limited_by_dest_capacity() {
    // available=100, rate=0.5, dest_capacity=10 -> actual=10, remaining=90
    let project = leak_project(100.0, 0.5, 10.0);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 10.0).abs() < 1e-6,
        "expected actual=10.0, got {actual}"
    );
    assert!(
        (remaining - 90.0).abs() < 1e-6,
        "expected remaining=90.0, got {remaining}"
    );
}

#[test]
fn test_leak_zero_available() {
    // available=0, rate=0.5, dest_capacity=INF -> actual=0, remaining=0
    let project = leak_project(0.0, 0.5, f64::INFINITY);
    let actual = run_and_get(&project, "out_actual");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (actual - 0.0).abs() < 1e-6,
        "expected actual=0.0, got {actual}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}

// ---------------------------------------------------------------------------
// systems_conversion tests
// ---------------------------------------------------------------------------

/// Build a project that instantiates systems_conversion with given inputs
/// and exposes "out_outflow", "out_waste", and "out_remaining" as outputs.
fn conversion_project(available: f64, rate: f64, dest_capacity: f64) -> datamodel::Project {
    let module_ident = "conv_mod";
    let model_name = "stdlib\u{205A}systems_conversion";

    datamodel::Project {
        name: "test_systems_conversion".to_string(),
        sim_specs: sim_specs(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                x_aux("available_val", &available.to_string()),
                x_aux("rate_val", &rate.to_string()),
                x_aux("dest_capacity_val", &dest_capacity.to_string()),
                systems_module(
                    module_ident,
                    model_name,
                    &[
                        ("available_val", "conv_mod.available"),
                        ("rate_val", "conv_mod.rate"),
                        ("dest_capacity_val", "conv_mod.dest_capacity"),
                    ],
                ),
                x_aux("out_outflow", "conv_mod.outflow"),
                x_aux("out_waste", "conv_mod.waste"),
                x_aux("out_remaining", "conv_mod.remaining"),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    }
}

#[test]
fn test_conversion_basic() {
    // available=10, rate=0.5, dest_capacity=INF -> outflow=INT(5.0)=5, waste=5, remaining=0
    let project = conversion_project(10.0, 0.5, f64::INFINITY);
    let outflow = run_and_get(&project, "out_outflow");
    let waste = run_and_get(&project, "out_waste");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (outflow - 5.0).abs() < 1e-6,
        "expected outflow=5.0, got {outflow}"
    );
    assert!(
        (waste - 5.0).abs() < 1e-6,
        "expected waste=5.0, got {waste}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}

#[test]
fn test_conversion_with_truncation() {
    // available=7, rate=0.3, dest_capacity=INF -> outflow=INT(2.1)=2, waste=5, remaining=0
    let project = conversion_project(7.0, 0.3, f64::INFINITY);
    let outflow = run_and_get(&project, "out_outflow");
    let waste = run_and_get(&project, "out_waste");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (outflow - 2.0).abs() < 1e-6,
        "expected outflow=2.0, got {outflow}"
    );
    assert!(
        (waste - 5.0).abs() < 1e-6,
        "expected waste=5.0, got {waste}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}

#[test]
fn test_conversion_limited_by_dest_capacity() {
    // available=10, rate=0.5, dest_capacity=3
    // Python reference: max_src_change = min(10, floor(3/0.5)) = min(10, 6) = 6
    //   outflow = floor(6 * 0.5) = 3
    //   waste = 6 - 3 = 3
    //   remaining = 10 - 6 = 4
    let project = conversion_project(10.0, 0.5, 3.0);
    let outflow = run_and_get(&project, "out_outflow");
    let waste = run_and_get(&project, "out_waste");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (outflow - 3.0).abs() < 1e-6,
        "expected outflow=3.0, got {outflow}"
    );
    assert!(
        (waste - 3.0).abs() < 1e-6,
        "expected waste=3.0, got {waste}"
    );
    assert!(
        (remaining - 4.0).abs() < 1e-6,
        "expected remaining=4.0, got {remaining}"
    );
}

#[test]
fn test_conversion_full_rate() {
    // available=10, rate=1.0, dest_capacity=INF -> outflow=10, waste=0, remaining=0
    let project = conversion_project(10.0, 1.0, f64::INFINITY);
    let outflow = run_and_get(&project, "out_outflow");
    let waste = run_and_get(&project, "out_waste");
    let remaining = run_and_get(&project, "out_remaining");
    assert!(
        (outflow - 10.0).abs() < 1e-6,
        "expected outflow=10.0, got {outflow}"
    );
    assert!(
        (waste - 0.0).abs() < 1e-6,
        "expected waste=0.0, got {waste}"
    );
    assert!(
        (remaining - 0.0).abs() < 1e-6,
        "expected remaining=0.0, got {remaining}"
    );
}
