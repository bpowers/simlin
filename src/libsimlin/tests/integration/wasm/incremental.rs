// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Persistent compilation through the public WASM FFI. Reuse is measured by
//! actual salsa body executions, not equal output or memo pointer identity.

use super::{parse_layout, run_and_stride, ParsedLayout};
use crate::common::{expect_no_error, open_project_from_datamodel};
use simlin::*;
use simlin_engine::{self as engine, db::exec_probe::ProbedDb, test_common::TestProject};
use std::{ffi::CStr, ptr};

struct Fixture {
    project: *mut SimlinProject,
    model: *mut SimlinModel,
}

impl Fixture {
    fn new(datamodel: &engine::datamodel::Project) -> Self {
        let project = open_project_from_datamodel(datamodel);
        unsafe {
            let mut error = ptr::null_mut();
            let model = simlin_project_get_model(project, ptr::null(), &mut error);
            expect_no_error(error, "get model");
            assert!(!model.is_null());
            Self { project, model }
        }
    }

    fn compile(&self, ltm: bool, discovery: bool) -> Result<(Vec<u8>, ParsedLayout), String> {
        unsafe {
            let (mut wasm, mut layout, mut error) =
                (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            let (mut wasm_len, mut layout_len) = (0, 0);
            simlin_model_compile_to_wasm(
                self.model,
                ltm,
                discovery,
                &mut wasm,
                &mut wasm_len,
                &mut layout,
                &mut layout_len,
                &mut error,
            );
            if !error.is_null() {
                assert!(wasm.is_null() && layout.is_null());
                assert_eq!((wasm_len, layout_len), (0, 0));
                let message = CStr::from_ptr(simlin_error_get_message(error))
                    .to_string_lossy()
                    .into_owned();
                simlin_error_free(error);
                return Err(message);
            }
            let blob = std::slice::from_raw_parts(wasm, wasm_len).to_vec();
            let parsed = parse_layout(std::slice::from_raw_parts(layout, layout_len));
            simlin_free(wasm);
            simlin_free(layout);
            Ok((blob, parsed))
        }
    }

    fn discovery(&self) -> bool {
        let db = unsafe { &*self.project }.lock_db();
        db.current_source_project()
            .unwrap()
            .ltm_discovery_mode(&*db)
    }

    fn set_discovery(&self, value: bool) {
        let mut db = unsafe { &*self.project }.lock_db();
        let source = db.current_source_project().unwrap();
        engine::db::set_project_ltm_discovery_mode(&mut db, source, value);
    }

    fn swap_db(&self, probe: &mut ProbedDb) {
        std::mem::swap(&mut *unsafe { &*self.project }.lock_db(), probe.db_mut());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            simlin_model_unref(self.model);
            simlin_project_unref(self.project);
        }
    }
}

fn feedback_model() -> engine::datamodel::Project {
    TestProject::new("wasm_incremental")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("rate", "0.1", None)
        .stock("level", "1", &["growth"], &[], None)
        .flow("growth", "rate * level", None)
        .build_datamodel()
}

/// Cover both overlay values and transition directions under each matching
/// project/call discovery setting. A differing per-call discovery override
/// changes salsa inputs and is tested for isolation below, not cache reuse.
/// The initial cold
/// call must populate THIS project's db; otherwise a throwaway compiler would
/// falsely look like zero query executions on every subsequent call.
#[test]
fn compile_to_wasm_reuses_project_queries_across_overlay_toggles() {
    for discovery in [false, true] {
        let datamodel = feedback_model();
        let fixture = Fixture::new(&datamodel);
        let mut probe = ProbedDb::new();
        // The same sync entry point project constructors use supplies real source
        // inputs; the probe changes only salsa's event sink, not the input shape.
        let source = probe.db_mut().sync(&datamodel);
        engine::db::set_project_ltm_discovery_mode(probe.db_mut(), source, discovery);
        for (ltm, cold) in [
            (false, true),
            (false, false),
            (true, true),
            (true, false),
            (false, false),
            (true, false),
        ] {
            probe.reset();
            fixture.swap_db(&mut probe);
            let result = fixture.compile(ltm, discovery);
            fixture.swap_db(&mut probe);
            let (_, layout) = result.unwrap();
            let counts = probe.counts();
            if cold {
                assert!(
                    counts
                        .get("assemble_simulation")
                        .is_some_and(|(runs, _)| *runs > 0),
                    "first overlay={ltm} compile must execute in the project db: {counts:?}"
                );
            } else {
                assert!(
                    counts.is_empty(),
                    "overlay={ltm} must reuse query results: {counts:?}"
                );
            }
            assert_eq!(
                layout
                    .var_offsets
                    .iter()
                    .any(|(name, _)| name.starts_with("$⁚ltm⁚")),
                ltm
            );
        }
    }
}

/// Every cross-product arm matters: the per-call discovery flag overrides the
/// project flag even with the overlay disabled, then restores either prior
/// value. Separate tests below cover compile and codegen failure paths.
#[test]
fn compile_to_wasm_discovery_flags_are_per_call() {
    let fixture = Fixture::new(&feedback_model());
    for prior in [false, true] {
        fixture.set_discovery(prior);
        for ltm in [false, true] {
            for discovery in [false, true] {
                let (blob, layout) = fixture.compile(ltm, discovery).unwrap();
                assert_eq!(fixture.discovery(), prior);
                let loop_columns = layout
                    .var_offsets
                    .iter()
                    .any(|(name, _)| name.starts_with("$⁚ltm⁚loop_score⁚"));
                assert_eq!(
                    loop_columns,
                    ltm && !discovery,
                    "ltm={ltm}, discovery={discovery}, prior={prior}"
                );
                let level = layout
                    .var_offsets
                    .iter()
                    .find(|(name, _)| name == "level")
                    .unwrap()
                    .1;
                assert_eq!(
                    run_and_stride(&blob, &layout, level),
                    vec![1.0, 1.1, 1.2100000000000002, 1.3310000000000002]
                );
            }
        }
    }
}

#[test]
fn compile_to_wasm_patch_invalidates_the_program() {
    let fixture = Fixture::new(&super::simple_model());
    let (before, before_layout) = fixture.compile(false, false).unwrap();
    let patch = br#"{"models":[{"name":"main","ops":[{"type":"upsertAux","payload":{"aux":{"name":"inflow_rate","equation":"3"}}}]}]}"#;
    unsafe {
        let (mut collected, mut error) = (ptr::null_mut(), ptr::null_mut());
        simlin_project_apply_patch(
            fixture.project,
            patch.as_ptr(),
            patch.len(),
            false,
            true,
            &mut collected,
            &mut error,
        );
        expect_no_error(error, "patch rate");
        if !collected.is_null() {
            simlin_error_free(collected);
        }
    }
    for ltm in [false, true] {
        let (after, layout) = fixture.compile(ltm, false).unwrap();
        let level = layout
            .var_offsets
            .iter()
            .find(|(name, _)| name == "level")
            .unwrap()
            .1;
        assert_eq!(
            *run_and_stride(&after, &layout, level).last().unwrap(),
            30.0
        );
    }
    let old_level = before_layout
        .var_offsets
        .iter()
        .find(|(name, _)| name == "level")
        .unwrap()
        .1;
    assert_eq!(
        *run_and_stride(&before, &before_layout, old_level)
            .last()
            .unwrap(),
        20.0
    );
}

/// Engine compilation and WASM lowering fail at different stages. Both must
/// restore either prior discovery flag and leave both FFI buffers unset.
#[test]
fn compile_to_wasm_errors_restore_discovery() {
    let invalid = TestProject::new("invalid")
        .aux("a", "missing_variable + 1", None)
        .build_datamodel();
    let unsupported = TestProject::new("unsupported")
        .with_sim_time(0.0, 1.0, 1.0)
        .indexed_dimension("A", 3)
        .array_aux("source[A]", "A")
        .scalar_aux("lo", "1")
        .scalar_aux("hi", "2")
        .scalar_aux("total", "SUM(source[lo:hi])")
        .build_datamodel();
    for datamodel in [&invalid, &unsupported] {
        let fixture = Fixture::new(datamodel);
        for prior in [false, true] {
            fixture.set_discovery(prior);
            for ltm in [false, true] {
                for discovery in [false, true] {
                    assert!(fixture.compile(ltm, discovery).is_err());
                    assert_eq!(fixture.discovery(), prior);
                }
            }
        }
    }
}

/// Enumerate both special-stock dispatch arms and the false/true/false request
/// sequence: warnings are absent before a request and remain latched afterward.
#[test]
fn compile_to_wasm_latches_special_stock_ltm_diagnostics() {
    for (xml, stock_type) in [
        (
            include_str!("../../../../../test/conveyors/minimal_conveyor.xmile"),
            "conveyor stock",
        ),
        (
            include_str!("../../../../../test/queues/queue_drain.xmile"),
            "queue stock",
        ),
    ] {
        let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes())).unwrap();
        let fixture = Fixture::new(&datamodel);
        for (request, expected) in [(false, false), (true, true), (false, true)] {
            fixture.compile(request, false).unwrap();
            unsafe {
                let mut error = ptr::null_mut();
                let diagnostics = simlin_project_get_errors(fixture.project, &mut error);
                expect_no_error(error, "get diagnostics");
                let mut degraded = false;
                if !diagnostics.is_null() {
                    let count = simlin_error_get_detail_count(diagnostics);
                    if count > 0 {
                        let details = std::slice::from_raw_parts(
                            simlin_error_get_details(diagnostics),
                            count,
                        );
                        degraded = details.iter().any(|detail| {
                            !detail.message.is_null() && {
                                let message = CStr::from_ptr(detail.message).to_string_lossy();
                                message.contains(stock_type) && message.contains("is degraded")
                            }
                        });
                    }
                    simlin_error_free(diagnostics);
                }
                assert_eq!(degraded, expected, "{stock_type}, request={request}");
            }
        }
    }
}
