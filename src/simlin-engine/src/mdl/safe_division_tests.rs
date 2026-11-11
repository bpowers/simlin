// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim's documented absolute denominator threshold, through the production
//! MDL reader and VM. Constructed boundary cases, not Vensim run data.
//! https://www.vensim.com/documentation/fn_xidz.html
use crate::db::{
    LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::mdl::{parse_mdl, project_to_mdl};
use crate::vm::Vm;

#[test]
fn vensim_safe_division_uses_the_absolute_threshold() {
    let threshold = 1e-6_f64;
    let magnitudes = [
        0.0,
        f64::from_bits(1),
        threshold.next_down(),
        threshold,
        threshold.next_up(),
        2.0,
    ];
    for magnitude in magnitudes {
        for denominator in [magnitude, -magnitude] {
            for (function, fallback) in [("ZIDZ", 0.0), ("XIDZ", 7.0)] {
                let tail = if function == "XIDZ" { ", 7" } else { "" };
                let source = format!(
                    "result = {function}(3, denominator{tail}) ~~|\ndenominator = {denominator:?} ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n"
                );
                let project = parse_mdl(&source).expect("MDL reads");
                for project in [
                    &project,
                    &parse_mdl(&project_to_mdl(&project).expect("MDL saves"))
                        .expect("saved MDL reads"),
                ] {
                    let mut db = SimlinDb::default();
                    let sync = sync_from_datamodel_incremental(&mut db, project, None);
                    let compiled =
                        compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
                            .expect("compiles");
                    let mut vm = Vm::new(compiled).expect("VM builds");
                    vm.run_to_end().expect("runs");
                    let series = crate::test_common::collect_results(&vm.into_results());
                    let expected = if denominator.abs() < threshold {
                        fallback
                    } else {
                        3.0 / denominator
                    };
                    assert!(
                        series["result"].iter().all(|value| *value == expected),
                        "{function} denominator={denominator:?}: {:?}, expected {expected}",
                        series["result"]
                    );
                }
            }
        }
    }
}

#[test]
fn importing_xidz_does_not_duplicate_argument_calls() {
    let project = parse_mdl(
        "result = XIDZ(RANDOM 0 1(), RANDOM 0 1(), RANDOM 0 1()) ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|"
    ).expect("MDL reads");
    let result = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "result")
        .expect("result");
    let crate::datamodel::Equation::Scalar(equation) = result.get_equation().expect("equation")
    else {
        panic!("scalar");
    };
    assert_eq!(equation.matches("UNIFORM(").count(), 3, "{equation}");
    assert!(equation.starts_with("XIDZ("), "{equation}");
}

/// The engine does not execute RANDOM functions; PREVIOUS gives executable
/// stateful operands. Inspect production bytecode as well as numerical output,
/// including the fallback whose value is not selected in this example.
#[test]
fn compiled_xidz_loads_each_stateful_argument_once_in_call_order() {
    use crate::bytecode::{BuiltinId, Opcode};
    let project = parse_mdl(
        "result = XIDZ(PREVIOUS(Time, 1), PREVIOUS(Time, 2), PREVIOUS(Time, 3)) ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|"
    ).expect("MDL reads");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled =
        compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off).expect("compiles");
    let code = &compiled.modules[&compiled.root].compiled_flows;
    // Compilation returns unfused bytecode; Vm::new performs fusion later.
    let fallbacks: Vec<f64> = code
        .code
        .windows(2)
        .filter_map(|ops| {
            if let [Opcode::LoadConstant { id }, Opcode::LoadPrev { .. }] = ops {
                Some(code.literals[*id as usize])
            } else {
                None
            }
        })
        .collect();
    assert_eq!(fallbacks, [1.0, 2.0, 3.0], "{:?}", code.code);
    assert_eq!(
        code.code
            .iter()
            .filter(|op| matches!(
                op,
                Opcode::Apply {
                    func: BuiltinId::Xidz
                }
            ))
            .count(),
        1
    );
    let mut vm = Vm::new(compiled).expect("VM builds");
    vm.run_to_end().expect("runs");
    let series = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(series["result"], [0.5, 0.0, 1.0]);
}

#[test]
fn exact_zero_safediv_keeps_tiny_denominator_results_on_mdl_roundtrip() {
    for denominator in [1e-7, -1e-7] {
        let source = format!(
            "result = SAFEDIV(3, {denominator}, 7) ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|"
        );
        let project = parse_mdl(&source).expect("MDL reads");
        let (saved, warnings) = crate::mdl::project_to_mdl_with_warnings(&project).expect("saves");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        for project in [&project, &parse_mdl(&saved).expect("saved reads")] {
            let mut db = SimlinDb::default();
            let sync = sync_from_datamodel_incremental(&mut db, project, None);
            let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
                .expect("compiles");
            let mut vm = Vm::new(compiled).expect("VM builds");
            vm.run_to_end().expect("runs");
            let series = crate::test_common::collect_results(&vm.into_results());
            assert!(
                series["result"]
                    .iter()
                    .all(|value| *value == 3.0 / denominator),
                "{:?}",
                series["result"]
            );
        }
    }
}
