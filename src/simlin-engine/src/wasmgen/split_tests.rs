// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Function partitioning through production compilation, with small byte targets
//! so runtime-state boundaries are exercised without production-sized fixtures.

use super::tests::{
    assert_matches_vm_nan_aware, compile_sim, layout_offset, qualified_ident, run_artifact_results,
    run_artifact_results_repeated, run_artifact_segmented, submodel_project,
};
use super::*;
use crate::bytecode::Opcode;
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, SimMethod};
use crate::test_common::TestProject;

/// Read section/vector lengths only; instruction validation stays with the wasm
/// interpreter. Inspecting the code section proves a test actually split its
/// target phases, rather than merely comparing two unsplit executions.
fn read_uleb(bytes: &[u8], cursor: &mut usize) -> usize {
    let mut result = 0;
    let mut shift = 0;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        result |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return result;
        }
        shift += 7;
        assert!(shift < 35, "malformed wasm length");
    }
}

fn function_bodies(artifact: &WasmArtifact) -> Vec<&[u8]> {
    let bytes = &artifact.wasm;
    assert_eq!(&bytes[..8], b"\0asm\x01\0\0\0");
    let mut cursor = 8;
    while cursor < bytes.len() {
        let section = bytes[cursor];
        cursor += 1;
        let size = read_uleb(bytes, &mut cursor);
        let end = cursor + size;
        if section == 10 {
            let count = read_uleb(bytes, &mut cursor);
            let bodies = (0..count)
                .map(|_| {
                    let len = read_uleb(bytes, &mut cursor);
                    let body = &bytes[cursor..cursor + len];
                    cursor += len;
                    body
                })
                .collect();
            assert_eq!(cursor, end, "code-section lengths must cover its payload");
            return bodies;
        }
        cursor = end;
    }
    panic!("generated module has no code section")
}

fn assert_same_slab(expected: &[f64], actual: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (i, (&expected, &actual)) in expected.iter().zip(actual).enumerate() {
        assert!(
            (expected.is_nan() && actual.is_nan()) || expected.to_bits() == actual.to_bits(),
            "slot {i}: unsplit={expected}, split={actual}"
        );
    }
}

fn split_and_unsplit(sim: &CompiledSimulation, target: usize) -> (WasmArtifact, WasmArtifact) {
    let unsplit = compile_with_passes(sim, &[], &[], None, usize::MAX).expect("unsplit wasm");
    let split = compile_with_passes(sim, &[], &[], None, target).expect("split wasm");
    assert_eq!(split.layout.serialize(), unsplit.layout.serialize());
    assert!(
        function_bodies(&split).len() > function_bodies(&unsplit).len(),
        "the small target must actually create function chunks"
    );
    (split, unsplit)
}

/// All integration methods call all three program phases. Distinct initial
/// literal pools and PREVIOUS/INIT values must survive those phase dispatchers;
/// neither restarting a run nor stopping between save points may change them.
#[test]
fn split_phases_preserve_integrators_snapshots_and_resumed_runs() {
    for method in [
        SimMethod::Euler,
        SimMethod::RungeKutta2,
        SimMethod::RungeKutta4,
    ] {
        let project = TestProject::new("split_phases")
            .with_sim_time(0.0, 3.0, 0.25)
            .with_save_step(0.5)
            .with_sim_method(method)
            .stock("first", "11", &["grow_first"], &[], None)
            .stock("second", "first + 23", &["grow_second"], &[], None)
            .stock("third", "second + 37", &["grow_third"], &[], None)
            .flow("grow_first", "first * 0.1", None)
            .flow("grow_second", "first + second * 0.01", None)
            .flow("grow_third", "second - third * 0.02", None)
            .scalar_aux("snapshot", "INIT(first) + INIT(second) + INIT(third)")
            .scalar_aux("lagged", "PREVIOUS(first, 101) + PREVIOUS(second, 211)")
            .scalar_aux(
                "nested",
                "IF TIME < 1 THEN first ELSE IF TIME < 2 THEN second ELSE third",
            )
            .build_datamodel();
        let sim = compile_sim(&project, "main");
        for target in [1, 128] {
            let (split, unsplit) = split_and_unsplit(&sim, target);
            let split_bodies = function_bodies(&split);
            let unsplit_bodies = function_bodies(&unsplit);
            let first_program = build_helpers().functions.len();
            for phase in [F_INITIALS, F_FLOWS, F_STOCKS] {
                let index = first_program + phase as usize;
                assert_ne!(
                    split_bodies[index], unsplit_bodies[index],
                    "phase {phase} must split"
                );
            }
            let expected = run_artifact_results(&unsplit);
            assert_same_slab(&expected, &run_artifact_results(&split));
            assert_same_slab(
                &expected,
                &run_artifact_segmented(&split, &[0.75, 1.75, 3.0]),
            );
            for rerun in run_artifact_results_repeated(&split, 2) {
                assert_same_slab(&expected, &rerun);
            }
            assert!(assert_matches_vm_nan_aware(sim.clone(), &split) >= 9);
        }
    }
}

/// Each shape is generated by the ordinary compiler, and its opcode family is
/// asserted below. The one-byte target attempts a cut at every legal boundary:
/// dynamic index and view locals must remain local to their helper,
/// while materialized temp arrays can cross helper calls in linear memory.
/// The hoisted loops also leave static descriptors for the end-of-program
/// discard rule. Dynamic descriptors in this fixture are consumed within the
/// program; it does not establish a producer that leaves dynamic descriptors
/// live at the final bytecode boundary.
#[test]
fn split_preserves_condition_subscript_view_and_temp_lifetimes() {
    let project = TestProject::new("split_arrays")
        .with_sim_time(0.0, 4.0, 1.0)
        .indexed_dimension("A", 5)
        .indexed_dimension("B", 3)
        .array_aux("source[A]", "A + TIME")
        .array_aux("matrix[A,B]", "A * 10 + B + TIME")
        .scalar_aux("idx", "TIME + 1")
        .scalar_aux("row", "TIME + 1")
        .scalar_aux("col", "TIME + 1")
        .scalar_aux("picked", "source[idx]")
        .scalar_aux("picked_matrix", "matrix[row,col]")
        .scalar_aux("view_sum", "SUM(matrix[row,1])")
        .scalar_aux("temp_sum", "SUM(2 * source[3:5] + 1)")
        .scalar_aux("transpose_sum", "SUM(matrix')")
        .array_aux("order[A]", "VECTOR SORT ORDER(source[A], -1)")
        .scalar_aux("order_sum", "SUM(order[*])")
        .scalar_aux(
            "nested",
            "IF TIME < 2 THEN picked ELSE IF TIME < 3 THEN temp_sum ELSE transpose_sum",
        )
        .build_datamodel();
    let sim = compile_sim(&project, "main");
    let code = &sim.modules[&sim.root].compiled_flows.code;
    for (name, present) in [
        (
            "condition",
            code.iter().any(|op| matches!(op, Opcode::SetCond {})),
        ),
        (
            "scalar index",
            code.iter()
                .any(|op| matches!(op, Opcode::PushSubscriptIndex { .. })),
        ),
        (
            "dynamic view",
            code.iter()
                .any(|op| matches!(op, Opcode::ViewSubscriptDynamic { .. })),
        ),
        (
            "temp iteration",
            code.iter().any(|op| matches!(op, Opcode::BeginIter { .. })),
        ),
        (
            "temp write",
            code.iter()
                .any(|op| matches!(op, Opcode::StoreIterElement {})),
        ),
        (
            "vector scratch",
            code.iter()
                .any(|op| matches!(op, Opcode::VectorSortOrder { .. })),
        ),
    ] {
        assert!(present, "production fixture must exercise {name}");
    }
    let (split, unsplit) = split_and_unsplit(&sim, 1);
    let expected = run_artifact_results(&unsplit);
    assert_same_slab(&expected, &run_artifact_results(&split));
    assert!(assert_matches_vm_nan_aware(sim, &split) >= 10);
    let matrix_off = layout_offset(&split, "picked_matrix");
    assert!(expected[matrix_off].is_finite());
    assert!(expected[4 * split.layout.n_slots + matrix_off].is_nan());
}

/// Two module parameters have distinct, time-varying values and are consumed
/// noncommutatively in both initial and flow equations. Two instances exercise
/// relative addressing and different input values through the same child body.
#[test]
fn split_module_dispatchers_preserve_inputs_and_instance_offsets() {
    for method in [
        SimMethod::Euler,
        SimMethod::RungeKutta2,
        SimMethod::RungeKutta4,
    ] {
        let mut project = submodel_project(
            "split_modules",
            method,
            "2 + TIME",
            "in + 10 * other + out * 0.01",
            true,
            2,
        );
        let mut additions = TestProject::new("inputs")
            .scalar_aux("other_value", "7 + TIME * 2")
            .build_datamodel();
        project.models[0]
            .variables
            .append(&mut additions.models[0].variables);
        for variable in &mut project.models[0].variables {
            if let datamodel::Variable::Module(module) = variable {
                module.references.push(datamodel::ModuleReference {
                    src: "other_value".to_string(),
                    dst: format!("{}.other", module.ident),
                });
                if module.ident == "sub1" {
                    module.references[0].src = "other_value".to_string();
                    module.references[1].src = "in_value".to_string();
                }
            }
        }
        let mut child_additions = TestProject::new("child")
            .scalar_aux("other", "-997")
            .stock("extra", "other + 30 * in", &["extra_grow"], &[], None)
            .flow("extra_grow", "other + out * 0.03", None)
            .scalar_aux("initial_out", "INIT(out)")
            .scalar_aux("previous_out", "PREVIOUS(out, -101)")
            .build_datamodel();
        if let datamodel::Variable::Aux(other) = &mut child_additions.models[0].variables[0] {
            other.compat.can_be_module_input = true;
        }
        project.models[1]
            .variables
            .append(&mut child_additions.models[0].variables);
        for variable in &mut project.models[1].variables {
            if let datamodel::Variable::Stock(stock) = variable
                && stock.ident == "out"
            {
                stock.equation = datamodel::Equation::Scalar("in + 10 * other".to_string());
            }
        }
        let sim = compile_sim(&project, "main");
        assert!(sim.modules.values().any(|module| {
            module
                .compiled_flows
                .code
                .iter()
                .any(|op| matches!(op, Opcode::LoadModuleInput { input: 1 }))
        }));
        let (split, unsplit) = split_and_unsplit(&sim, 1);
        let expected = run_artifact_results(&unsplit);
        assert_same_slab(&expected, &run_artifact_results(&split));
        assert!(assert_matches_vm_nan_aware(sim, &split) >= 10);
        assert_eq!(
            expected[layout_offset(&split, qualified_ident("sub0", "out").as_str())],
            72.0
        );
        assert_eq!(
            expected[layout_offset(&split, qualified_ident("sub1", "out").as_str())],
            27.0
        );
    }
}

/// Special stocks take the production expansion dispatch. Their reconciliation
/// initials form a fourth emitted program variant and must split without
/// reinitializing the container slots the side-table pass has just published.
#[test]
fn split_preserves_special_stock_reconciliation_initials() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>split_queue</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>40</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <aux name="init_sum"><eqn>INIT(SUM(waiting))</eqn></aux>
    <aux name="init_size"><eqn>INIT(SIZE(waiting))</eqn></aux>
    <aux name="ratio"><eqn>SUM(waiting) / INIT(SUM(waiting))</eqn></aux>
  </variables></model>
</xmile>"#;
    let queue = crate::xmile::project_from_reader(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("queue model");
    let conveyor = crate::xmile::project_from_reader(&mut std::io::BufReader::new(
        include_str!("../../../../test/conveyors/conveyor_containers.xmile").as_bytes(),
    ))
    .expect("conveyor model");
    for project in [queue, conveyor] {
        let main = project.models[0].name.as_str();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let built = crate::queue_compile::compile_sim(
            &mut db,
            sync.project,
            &project,
            main,
            crate::db::LtmOverlay::Off,
        )
        .expect("special-stock production dispatch");
        let unsplit = compile_with_passes(
            &built.compiled,
            &built.conveyor_plans,
            &built.queue_plans,
            None,
            usize::MAX,
        )
        .expect("unsplit special wasm");
        let split = compile_with_passes(
            &built.compiled,
            &built.conveyor_plans,
            &built.queue_plans,
            None,
            1,
        )
        .expect("split special wasm");
        let plain_bodies = function_bodies(&unsplit);
        let split_bodies = function_bodies(&split);
        assert!(split_bodies.len() > plain_bodies.len());
        let reconciliation_index = plain_bodies.len() - 1;
        assert_ne!(
            plain_bodies[reconciliation_index], split_bodies[reconciliation_index],
            "reconciliation initials must split"
        );
        let expected = run_artifact_results(&unsplit);
        assert_same_slab(&expected, &run_artifact_results(&split));
        for rerun in run_artifact_results_repeated(&split, 2) {
            assert_same_slab(&expected, &rerun);
        }
        let mut vm = crate::queue_compile::build_vm(&project, main).expect("special-stock VM");
        vm.run_to_end().expect("VM run");
        let vm = vm.into_results();
        assert_eq!(vm.step_count, split.layout.n_chunks);
        let mut compared = 0;
        for (name, wasm_off) in &split.layout.var_offsets {
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(&vm_off) = vm.offsets.get(&ident) else {
                continue;
            };
            for row in 0..vm.step_count {
                let left = vm.data[row * vm.step_size + vm_off];
                let right = expected[row * split.layout.n_slots + wasm_off];
                assert!(
                    (left.is_nan() && right.is_nan()) || (left - right).abs() < 1e-9,
                    "{name}, row {row}: VM={left}, wasm={right}"
                );
            }
            compared += 1;
        }
        assert!(compared >= 5);
    }
}
