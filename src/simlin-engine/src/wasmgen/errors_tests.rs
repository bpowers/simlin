// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the emitted module's runtime error channel ([`super`]). Split out of
//! `errors.rs` to keep that file under the project line-count lint; this is the
//! `#[cfg(test)] mod tests` body, included via `#[path]` so `use super::*` still
//! resolves the module's private items.
//!
//! Two halves, matching the channel's two halves:
//!
//! * **The emitted side.** No production model can raise yet (the queue pass has
//!   no per-step runtime error, and the conveyor belt pass is not lowered), so the
//!   tests splice `FaultInjection` -- a synthetic pass that raises on command --
//!   into the same two hook points `passes::QueuePass` occupies. The modules run
//!   under the DLR-FT interpreter, exactly as the rest of `wasmgen`'s tests do.
//! * **The host side.** `reconstruct_error` must rebuild the bytecode VM's message
//!   character for character. The oracle is the VM itself: the same conveyor model
//!   is run through `build_vm` under a shrunken `SlatBoundGuard`, and the error it
//!   returns is compared to the reconstruction.

use super::*;

use crate::common::ErrorCode;
use crate::conveyor::SlatBoundGuard;
use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use crate::queue_compile::{build_vm, compile_sim};
use crate::vm::{CompiledSimulation, Vm};
use crate::wasmgen::WasmArtifact;
use crate::wasmgen::module::{compile_simulation, compile_simulation_with_fault};
use checked::{Store, Stored, StoredValue};
use std::io::BufReader;
use wasm::addrs::ModuleAddr;
use wasm::validate;

type TestStore<'a> = Store<'a, ()>;
type Inst = Stored<ModuleAddr>;

// ── drivers ──────────────────────────────────────────────────────────────────

fn compile(datamodel: &crate::datamodel::Project) -> CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    compile_project_incremental(&db, sync.project, "main").expect("incremental compile")
}

fn with_instance<R>(
    artifact: &WasmArtifact,
    body: impl FnOnce(&mut TestStore<'_>, Inst) -> R,
) -> R {
    let info = validate(&artifact.wasm).expect("generated module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    body(&mut store, inst)
}

fn call_void(store: &mut TestStore<'_>, inst: Inst, name: &str) {
    let f = store
        .instance_export(inst, name)
        .unwrap_or_else(|_| panic!("{name} export"))
        .as_func()
        .unwrap_or_else(|| panic!("{name} is a function"));
    store
        .invoke_simple_typed::<(), ()>(f, ())
        .unwrap_or_else(|_| panic!("invoke {name}"));
}

fn call_run_to(store: &mut TestStore<'_>, inst: Inst, target: f64) {
    let f = store
        .instance_export(inst, "run_to")
        .expect("run_to export")
        .as_func()
        .expect("run_to is a function");
    store
        .invoke_simple_typed::<(f64,), ()>(f, (target,))
        .expect("invoke run_to");
}

/// The raw `get_error()` word.
fn get_error(store: &mut TestStore<'_>, inst: Inst) -> i64 {
    let f = store
        .instance_export(inst, "get_error")
        .expect("get_error export")
        .as_func()
        .expect("get_error is a function");
    store
        .invoke_simple_typed::<(), i64>(f, ())
        .expect("invoke get_error")
}

/// The live saved-row counter (`G_SAVED`), exported as `saved_steps`.
fn saved_steps(store: &mut TestStore<'_>, inst: Inst) -> u32 {
    let g = store
        .instance_export(inst, "saved_steps")
        .expect("saved_steps export")
        .as_global()
        .expect("saved_steps is a global");
    match store.global_read(g) {
        StoredValue::I32(x) => x,
        other => panic!("expected i32 global, got {other:?}"),
    }
}

/// One f64 out of the live `curr` chunk (base 0).
fn curr_slot(store: &mut TestStore<'_>, inst: Inst, off: usize) -> f64 {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
    store.mem_access_mut_slice(mem, |bytes| {
        let a = off * 8;
        f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
    })
}

/// Poke one f64 into the live `curr` chunk, so a test can watch whether a
/// subsequent call rewrites it.
fn set_curr_slot(store: &mut TestStore<'_>, inst: Inst, off: usize, value: f64) {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
    store.mem_access_mut_slice(mem, |bytes| {
        let a = off * 8;
        bytes[a..a + 8].copy_from_slice(&value.to_le_bytes());
    });
}

/// The whole step-major results slab.
fn read_slab(store: &mut TestStore<'_>, inst: Inst, artifact: &WasmArtifact) -> Vec<f64> {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
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

fn layout_offset(artifact: &WasmArtifact, name: &str) -> usize {
    artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name} in layout"))
        .1
}

// ── fixtures ─────────────────────────────────────────────────────────────────

/// A minimal scalar Euler model: `level` integrates a constant `inflow_rate` of 2
/// from t=0 to t=5 at dt=1, so `run` saves six rows (t = 0..=5) whose `level`
/// values are `0, 2, 4, 6, 8, 10`.
fn ramp_fixture() -> crate::datamodel::Project {
    crate::test_common::TestProject::new("errchan")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel()
}

/// A conveyor whose transit time needs more slats than a shrunken bound: the
/// fixture `conveyor_compile_tests` uses for the init-time slat-bound rejection.
/// At dt=0.25 a transit of 1.25 is 5 slats, one over a bound of 4.
fn over_bound_conveyor_xmile(len_eqn: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
<options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>20</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>{len_eqn}</len></conveyor></stock>
    <flow name="in_f"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
  </variables></model>
</xmile>"#
    )
}

/// The conveyor plan list `queue_compile::compile_sim` resolves for `project`.
/// These are the SAME plans the VM attaches, so a reconstruction driven by them is
/// driven by the belt names and slot offsets the VM used.
fn conveyor_plans(
    project: &crate::datamodel::Project,
) -> Vec<crate::conveyor_compile::ConveyorPlan> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_sim(&mut db, sync.project, project, "main")
        .expect("conveyor model must build")
        .conveyor_plans
}

/// The packed `get_error` word for a `(code, belt)` pair, built the way the blob
/// builds it. Lets the host-side tests exercise `decode`/`reconstruct` without an
/// emitted module that can raise.
fn error_word(code: ErrorCode, belt: usize) -> i64 {
    ((belt as u32 as i64) << 32) | (code as i32 as u32 as i64)
}

// ── the packing contract ─────────────────────────────────────────────────────

// The round trip against the REAL packing is asserted end to end by the
// `reconstruct_*` tests below, which read a running blob's `get_error()` and
// rebuild the VM's message (belt index included) from the decoded word. What is
// left here is the one thing an emitted blob cannot produce: a word whose code
// half is zero.

/// A zero CODE means "no error", even when the belt half carries a stale index --
/// which is why the decoder tests the low word rather than the whole i64.
#[test]
fn error_word_zero_code_is_no_error() {
    assert_eq!(decode_error_word(0), None);
    assert_eq!(decode_error_word(error_word(ErrorCode::NoError, 7)), None);
}

// ── host-side message reconstruction ─────────────────────────────────────────

/// The headline acceptance criterion, minus the belt pass: given only the code,
/// the belt index, the plan list, and `curr[len_off]`, the host rebuilds the
/// bytecode VM's `ConveyorTransitTooLong` message CHARACTER FOR CHARACTER.
///
/// The VM is the oracle: the same model, under the same `SlatBoundGuard`, is run
/// through `build_vm` and its error text captured. Nothing in this test writes the
/// expected string by hand -- see `reconstruct_error_matches_literal_vm_format`
/// for the literal-format pin that keeps this from being a tautology.
#[test]
fn reconstruct_transit_too_long_matches_vm_message() {
    let _guard = SlatBoundGuard::new(4);
    let project = crate::xmile::project_from_reader(&mut BufReader::new(
        over_bound_conveyor_xmile("1.25").as_bytes(),
    ))
    .expect("parse xmile");

    // The VM's error, from the very code path the blob will replace.
    let mut vm = build_vm(&project, "main").expect("build");
    let vm_err = vm
        .run_to_end()
        .expect_err("5 slats against a bound of 4 must be rejected");
    assert_eq!(vm_err.code, ErrorCode::ConveyorTransitTooLong);
    let vm_message = vm_err
        .get_details()
        .expect("the VM error carries a message");

    // The host's reconstruction, from the plan + the transit slot the blob leaves
    // in `curr`.
    let plans = conveyor_plans(&project);
    assert_eq!(plans.len(), 1, "one belt");
    let word = error_word(ErrorCode::ConveyorTransitTooLong, 0);
    let (code, message) = reconstruct_error(word, &plans, 0.25, |off| {
        assert_eq!(off, plans[0].len_off, "only the transit slot is read");
        1.25
    })
    .expect("a raised error reconstructs");

    assert_eq!(code, ErrorCode::ConveyorTransitTooLong);
    assert_eq!(message, vm_message);
}

/// The same reconstruction, pinned against the LITERAL format string rather than
/// against the VM. Without this, `reconstruct_transit_too_long_matches_vm_message`
/// would still pass if both sides regressed to the empty string.
#[test]
fn reconstruct_error_matches_literal_vm_format() {
    let _guard = SlatBoundGuard::new(4);
    let project = crate::xmile::project_from_reader(&mut BufReader::new(
        over_bound_conveyor_xmile("1.25").as_bytes(),
    ))
    .expect("parse xmile");
    let plans = conveyor_plans(&project);

    let (_, message) = reconstruct_error(
        error_word(ErrorCode::ConveyorTransitTooLong, 0),
        &plans,
        0.25,
        |_| 1.25,
    )
    .expect("reconstructs");
    assert_eq!(
        message,
        "conveyor 'belt' transit time 1.25 at dt 0.25 needs 5 belt slats, \
         exceeding the maximum of 4"
    );
}

/// `init_belts` raises `ConveyorTransitNotPositive` as well as
/// `ConveyorTransitTooLong`, so the reconstruction table covers both. A `<len>` of
/// `TIME` is 0 at t=0, which the belt init rejects.
#[test]
fn reconstruct_transit_not_positive_matches_vm_message() {
    let project = crate::xmile::project_from_reader(&mut BufReader::new(
        over_bound_conveyor_xmile("TIME").as_bytes(),
    ))
    .expect("parse xmile");

    let mut vm = build_vm(&project, "main").expect("build");
    let vm_err = vm
        .run_to_end()
        .expect_err("a transit of 0 at t=0 must be rejected");
    assert_eq!(vm_err.code, ErrorCode::ConveyorTransitNotPositive);
    let vm_message = vm_err.get_details().expect("message");

    let plans = conveyor_plans(&project);
    let (code, message) = reconstruct_error(
        error_word(ErrorCode::ConveyorTransitNotPositive, 0),
        &plans,
        0.25,
        |_| 0.0,
    )
    .expect("reconstructs");
    assert_eq!(code, ErrorCode::ConveyorTransitNotPositive);
    assert_eq!(message, vm_message);
    assert_eq!(
        message,
        "conveyor 'belt' transit time must be positive and finite, got 0"
    );
}

/// A blob and a host that disagree about the channel surface loudly rather than
/// panicking or silently reporting success.
#[test]
fn reconstruct_error_is_total_on_a_bad_channel() {
    let project = crate::xmile::project_from_reader(&mut BufReader::new(
        over_bound_conveyor_xmile("1").as_bytes(),
    ))
    .expect("parse xmile");
    let plans = conveyor_plans(&project);

    // Belt index past the plan list.
    let (code, message) = reconstruct_error(
        error_word(ErrorCode::ConveyorTransitTooLong, 9),
        &plans,
        0.25,
        |_| 1.0,
    )
    .expect("a nonzero code always yields something");
    assert_eq!(code, ErrorCode::Generic);
    assert!(message.contains("belt 9"), "{message}");

    // A code the reconstruction table does not know.
    let (code, message) =
        reconstruct_error(error_word(ErrorCode::BadTable, 0), &plans, 0.25, |_| 1.0)
            .expect("a nonzero code always yields something");
    assert_eq!(code, ErrorCode::Generic);
    assert!(message.contains("unknown runtime error code"), "{message}");

    // No error: no reconstruction.
    assert!(reconstruct_error(0, &plans, 0.25, |_| 1.0).is_none());
}

// ── the emitted channel: an ordinary model ───────────────────────────────────

/// Every blob exports `get_error`, and a model whose passes cannot raise reports
/// 0 after a full run. This is what lets a host poll the channel unconditionally,
/// with no feature detection.
#[test]
fn ordinary_model_exports_get_error_and_reports_no_error() {
    let datamodel = ramp_fixture();
    let artifact = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    with_instance(&artifact, |store, inst| {
        assert_eq!(get_error(store, inst), 0, "before any run");
        call_void(store, inst, "run");
        assert_eq!(get_error(store, inst), 0, "after a clean run");
    });
}

// ── the emitted channel: a raising step ──────────────────────────────────────

/// A pass that raises between Flows and Stocks reports its code and belt, and the
/// FAILING STEP SAVES NO ROW: `run_to` returns before the Stocks phase, the
/// `prev_values` snapshot, and the save/advance tail, exactly as `vm.rs` returns
/// `Err` before the same three. Rows recorded by the steps that DID complete are
/// intact and equal the VM's.
#[test]
fn step_fault_reports_error_and_saves_no_row() {
    let datamodel = ramp_fixture();
    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitTooLong,
            belt: 2,
            site: FaultSite::Step { at_time: 3.0 },
            marker: None,
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    let level_off = layout_offset(&artifact, "level");
    let n_slots = artifact.layout.n_slots;

    // The VM's clean series, as the oracle for the rows that did complete.
    let mut vm = Vm::new(compile(&datamodel)).expect("vm");
    vm.run_to_end().expect("vm runs the fault-free model");
    let vm_level = vm.get_series(&crate::common::Ident::new("level")).unwrap();

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");

        let decoded = decode_error_word(get_error(store, inst)).expect("the step raised");
        assert_eq!(decoded.code, ErrorCode::ConveyorTransitTooLong as i32);
        assert_eq!(decoded.belt, 2);

        // Steps at t = 0, 1, 2 completed and saved; the step at t = 3 raised.
        assert_eq!(saved_steps(store, inst), 3);
        let slab = read_slab(store, inst, &artifact);
        for row in 0..3 {
            assert_eq!(slab[row * n_slots + level_off], vm_level[row], "row {row}");
        }
        // The failing step never wrote its row: the slab keeps its zero-init.
        assert_eq!(slab[3 * n_slots + level_off], 0.0, "no row for the failure");
        // The VM would have written 6 here, so a saved row would have been visible.
        assert_eq!(vm_level[3], 6.0);
    });
}

/// The unwind contract (`errors::ErrorScope`): a raise branches clean out of the
/// PASS BLOCK, so nothing later in the pass body runs -- the wasm equivalent of the
/// VM's `?` abandoning the remaining iterations of `run_phase_a`. `later_belt_marker`
/// stands in for a subsequent belt's contribution; if the `br` were one label short
/// it would merely leave the conditional and fall through to that store.
///
/// Asserted at both hook points, because `ErrorScope::raise` is emitted at two
/// different label depths there (0 in the init hook, 1 inside the step's `if`).
#[test]
fn a_raise_abandons_the_rest_of_the_pass_body() {
    let datamodel = ramp_fixture();
    let probe = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    let level_off = layout_offset(&probe, "level");

    for (site, expected_level) in [
        // Raised from the init hook (label depth 0): the initials left `level` at 0.
        (FaultSite::Initials, 0.0),
        // Raised from inside the step hook's `if` (label depth 1): the step at t=3
        // begins with `level` already advanced to 6.
        (FaultSite::Step { at_time: 3.0 }, 6.0),
    ] {
        let artifact = compile_simulation_with_fault(
            &compile(&datamodel),
            FaultInjection {
                code: ErrorCode::ConveyorTransitTooLong,
                belt: 0,
                site,
                marker: None,
                later_belt_marker: Some((level_off, 777.0)),
                publish_marker: None,
                needs_flows: false,
                init_probe: None,
            },
        )
        .expect("wasm codegen with fault");

        with_instance(&artifact, |store, inst| {
            call_void(store, inst, "run");
            assert_ne!(get_error(store, inst), 0, "{site:?} must raise");
            assert_eq!(
                curr_slot(store, inst, level_off),
                expected_level,
                "{site:?}: the pass body after the raise must not execute"
            );
        });
    }
}

/// `run_initials` is a no-op while the channel is set, so a host cannot re-enter a
/// half-built side table (a second belt init would bump-allocate a fresh set of
/// belt state on top of the abandoned one). Observed by poking the marker slot back
/// and checking the second call does not rewrite it.
#[test]
fn run_initials_is_a_no_op_while_the_channel_is_set() {
    let datamodel = ramp_fixture();
    let probe = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    let level_off = layout_offset(&probe, "level");

    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitNotPositive,
            belt: 0,
            site: FaultSite::Initials,
            marker: Some((level_off, 999.0)),
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        assert_ne!(get_error(store, inst), 0);
        assert_eq!(
            curr_slot(store, inst, level_off),
            999.0,
            "the pass ran once"
        );

        set_curr_slot(store, inst, level_off, -1.0);
        call_void(store, inst, "run_initials");
        assert_eq!(
            curr_slot(store, inst, level_off),
            -1.0,
            "the sticky channel kept run_initials from re-running the pass"
        );
    });
}

/// The channel is sticky: a second `run_to` after a raised error does nothing --
/// no extra rows, no re-attempt -- until `reset` clears it. `reset` then restores a
/// runnable blob (the deliberate divergence from the VM, which re-attempts; see
/// `errors::emit_return_if_error`).
///
/// Row counts alone would not pin this: a `run_to` that DID re-enter the loop would
/// re-raise at the same step and still save nothing. The observable is
/// `publish_marker`, stamped from the step-start container publish -- the first
/// thing a step does. Poking that slot after the error and finding it untouched
/// proves `run_to` returned at its post-`run_initials` guard rather than stepping.
/// `inflow` is the marker slot because the Flows phase overwrites it immediately
/// after the publish, so a stamped 888 can never reach the Stocks phase and perturb
/// a step that completes.
#[test]
fn step_fault_is_sticky_until_reset() {
    let datamodel = ramp_fixture();
    let probe = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    let inflow_off = layout_offset(&probe, "inflow");

    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitTooLong,
            belt: 0,
            site: FaultSite::Step { at_time: 3.0 },
            marker: None,
            later_belt_marker: None,
            publish_marker: Some((inflow_off, 888.0)),
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_ne!(get_error(store, inst), 0);
        assert_eq!(saved_steps(store, inst), 3);

        // Sticky: further stepping is refused, so no step begins -- not even its
        // container publish -- and the cursor cannot advance.
        set_curr_slot(store, inst, inflow_off, -1.0);
        call_run_to(store, inst, 5.0);
        assert_eq!(
            curr_slot(store, inst, inflow_off),
            -1.0,
            "run_to must not begin a step while the channel is set"
        );
        assert_eq!(saved_steps(store, inst), 3, "no rows added after the error");
        assert_ne!(get_error(store, inst), 0, "the error is still reported");

        // `run_initials` is likewise a no-op while the channel is set.
        call_void(store, inst, "run_initials");
        assert_ne!(get_error(store, inst), 0);

        // `reset` is the recovery: the channel clears and the cursor rewinds.
        call_void(store, inst, "reset");
        assert_eq!(get_error(store, inst), 0);
        assert_eq!(saved_steps(store, inst), 0);
    });
}

// ── the emitted channel: a raising side-table init ───────────────────────────

/// A side-table init that raises returns from `run_initials` before the container
/// publish, before the reconciliation, and before arming the step cursor / setting
/// `G_DID_INITIALS` -- the wasm twin of `vm.rs:1542-1548`, where `init_belts`' `Err`
/// returns ahead of `self.did_initials = true`. A subsequent `run_to` then steps
/// nothing, so no Flows phase and no container publish ever run over a side table
/// that was never built.
///
/// `publish_marker` is the observable for that last part: it is stamped from the
/// step-start publish hook, so finding the slot untouched proves neither
/// `run_initials` nor `run_to` fell through to it.
#[test]
fn initials_fault_reports_error_and_saves_nothing() {
    let datamodel = ramp_fixture();
    let probe = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    let rate_off = layout_offset(&probe, "inflow_rate");

    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitNotPositive,
            belt: 0,
            site: FaultSite::Initials,
            marker: None,
            later_belt_marker: None,
            publish_marker: Some((rate_off, 888.0)),
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        let decoded = decode_error_word(get_error(store, inst)).expect("the init raised");
        assert_eq!(decoded.code, ErrorCode::ConveyorTransitNotPositive as i32);
        assert_eq!(decoded.belt, 0);
        assert_eq!(saved_steps(store, inst), 0);
        assert_eq!(
            curr_slot(store, inst, rate_off),
            0.0,
            "run_initials must not publish from an unbuilt side table"
        );

        // `run_to` sees the live channel (via its post-`run_initials` guard) and
        // steps nothing -- no publish, no Flows, no rows.
        call_run_to(store, inst, 5.0);
        assert_eq!(saved_steps(store, inst), 0, "no rows after a failed init");
        assert_eq!(
            curr_slot(store, inst, rate_off),
            0.0,
            "run_to must not enter the step loop after a failed init"
        );
        let slab = read_slab(store, inst, &artifact);
        assert!(
            slab.iter().all(|v| *v == 0.0),
            "the results slab is untouched"
        );
    });
}

/// A pass that declares `needs_flows_before_init` sees the Flows phase's output in
/// `curr` when its init hook runs -- the wasm twin of `vm.rs:1530-1541`, which
/// evaluates Flows before `init_belts` so the synthesized belt-parameter auxes
/// (transit, capacity, leak fractions) hold their real values rather than 0.
///
/// `inflow_rate` is the observable: it is an aux nothing in the model depends on
/// for a stock initial, so the initials runlist never evaluates it and it reads 0
/// until Flows runs. The probe copies it into `level` at the init hook. Both
/// branches of the flag are exercised, because a test that only asserted the
/// `true` case would still pass if the driver ran Flows unconditionally -- which
/// would be its own bug (a wasted evaluation, and one the VM does not make for a
/// queue-only model).
#[test]
fn flows_run_before_the_init_hook_only_when_a_pass_needs_them() {
    let datamodel = ramp_fixture();
    let probe = compile_simulation(&compile(&datamodel)).expect("wasm codegen");
    let level_off = layout_offset(&probe, "level");
    let rate_off = layout_offset(&probe, "inflow_rate");

    for (needs_flows, expected) in [(true, 2.0), (false, 0.0)] {
        let artifact = compile_simulation_with_fault(
            &compile(&datamodel),
            FaultInjection {
                code: ErrorCode::ConveyorTransitNotPositive,
                belt: 0,
                site: FaultSite::Initials,
                marker: None,
                later_belt_marker: None,
                publish_marker: None,
                needs_flows,
                init_probe: Some((level_off, rate_off)),
            },
        )
        .expect("wasm codegen with fault");

        with_instance(&artifact, |store, inst| {
            call_void(store, inst, "run_initials");
            assert_ne!(get_error(store, inst), 0, "the init hook ran and raised");
            assert_eq!(
                curr_slot(store, inst, level_off),
                expected,
                "needs_flows = {needs_flows}: the init hook's view of a Flows-only slot"
            );
        });
    }
}

/// The Flows evaluation `needs_flows_before_init` inserts must not disturb the
/// rest of the run: this pins whole-run VM parity for a model whose driver emits
/// the extra Flows call. (The snapshot-ordering property itself -- the call lands
/// after `initial_values` is captured, mirroring vm.rs -- is enforced by the
/// driver's emit order and documented on `emit_run_initials`.) The fault here
/// never raises, so the run completes and can be diffed against the VM.
#[test]
fn an_init_flows_evaluation_leaves_the_run_at_vm_parity() {
    let datamodel = ramp_fixture();
    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitTooLong,
            belt: 0,
            // Armed past `stop`, so the pass never raises: only the Flows call the
            // flag inserts is under test.
            site: FaultSite::Step { at_time: 1e9 },
            marker: None,
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: true,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    let mut vm = Vm::new(compile(&datamodel)).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_level = vm.get_series(&crate::common::Ident::new("level")).unwrap();

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_eq!(get_error(store, inst), 0, "nothing raised");
        assert_eq!(saved_steps(store, inst), 6);

        let level_off = layout_offset(&artifact, "level");
        let n_slots = artifact.layout.n_slots;
        let slab = read_slab(store, inst, &artifact);
        for (row, expected) in vm_level.iter().enumerate() {
            assert_eq!(slab[row * n_slots + level_off], *expected, "row {row}");
        }
    });
}

/// A pass emitter that reaches for an unwind block the driver never opened fails
/// LOUDLY at emit time. Emitting the `br` anyway would validate -- resolving to
/// whatever construct encloses the site, `run_to`'s step loop -- and hang the blob.
/// `expect_scope` is the only defense against that mistake, and it holds in a
/// release build.
#[test]
#[should_panic(expected = "no unwind block")]
fn raising_without_an_unwind_block_panics_at_emit_time() {
    super::expect_scope(None);
}

/// `run` delegates `reset; run_to(stop)`, and `reset` clears the channel, so an
/// init fault reports FRESHLY on every `run` -- the sticky guard never wedges a
/// blob a host is re-running from scratch.
///
/// The interleaved `reset` + assert-zero is what makes this non-vacuous: without
/// it, a second `run` that did nothing at all would still leave the first run's
/// error word in place and the test would pass on a stale read.
#[test]
fn initials_fault_reraises_on_each_run() {
    let datamodel = ramp_fixture();
    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitNotPositive,
            belt: 3,
            site: FaultSite::Initials,
            marker: None,
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        let decoded = decode_error_word(get_error(store, inst)).expect("the first run raises");
        assert_eq!(decoded.belt, 3);
        assert_eq!(saved_steps(store, inst), 0);

        // Prove the channel is empty before the second attempt, so a nonzero read
        // afterwards can only come from a fresh raise.
        call_void(store, inst, "reset");
        assert_eq!(get_error(store, inst), 0);

        call_void(store, inst, "run");
        let decoded = decode_error_word(get_error(store, inst)).expect("the second run re-raises");
        assert_eq!(decoded.belt, 3);
        assert_eq!(saved_steps(store, inst), 0);
    });
}

// ── the emitted channel: the mid-run preview swallows ────────────────────────

/// A fault that fires only in the mid-run PREVIEW is swallowed: `run_to` returns
/// cleanly, the channel reads 0, and the half-written pass output is rolled back
/// from the `curr` snapshot -- mirroring `vm.rs:1187-1216`, where the preview's
/// error restores the plain post-Flows chunk and is deliberately not propagated.
///
/// The fault is armed at `t = 2`, one dt past the `run_to(1.0)` target: the loop
/// breaks at `curr[TIME] = 2` without ever running that step, and the preview then
/// evaluates the pass at that resting time.
#[test]
fn preview_fault_is_swallowed_and_curr_is_restored() {
    let datamodel = ramp_fixture();
    // `inflow_rate` is a constant the Flows phase re-establishes every step, so a
    // marker stamped over it is unambiguously the pass's own half-written output.
    // Slot offsets come from the compiled sim, not from the fault, so a fault-free
    // artifact reports the same one (pinned below).
    let rate_off = layout_offset(
        &compile_simulation(&compile(&datamodel)).expect("wasm codegen"),
        "inflow_rate",
    );
    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitTooLong,
            belt: 0,
            site: FaultSite::Step { at_time: 2.0 },
            marker: Some((rate_off, 999.0)),
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");
    assert_eq!(rate_off, layout_offset(&artifact, "inflow_rate"));

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 1.0);

        // Steps at t = 0 and t = 1 completed; the loop broke at t = 2.
        assert_eq!(saved_steps(store, inst), 2);
        assert_eq!(curr_slot(store, inst, 0), 2.0, "resting time");

        // The preview raised, and the channel was cleared again.
        assert_eq!(get_error(store, inst), 0, "a preview failure is swallowed");
        // The marker the aborted pass stamped was rolled back to the Flows value.
        assert_eq!(
            curr_slot(store, inst, rate_off),
            2.0,
            "curr restored from the preview snapshot"
        );

        // Resuming re-runs the same step for real, and THERE the error is loud --
        // the behavior `vm.rs` documents when the inputs have not changed.
        call_run_to(store, inst, 5.0);
        let decoded = decode_error_word(get_error(store, inst)).expect("the resumed step raises");
        assert_eq!(decoded.code, ErrorCode::ConveyorTransitTooLong as i32);
        assert_eq!(saved_steps(store, inst), 2, "the failing step saved no row");
    });
}

/// The whole error-channel scaffolding -- the unwind block, the driver guards, the
/// preview's snapshot and restore -- is inert on a run that raises nothing: the
/// model simulates exactly as it does without a pass. Armed past `stop`, the fault
/// never fires. This is what keeps the swallow test above from passing vacuously
/// (a scaffolding that corrupted `curr` would show up here).
#[test]
fn an_unraised_fault_leaves_the_run_untouched() {
    let datamodel = ramp_fixture();
    let artifact = compile_simulation_with_fault(
        &compile(&datamodel),
        FaultInjection {
            code: ErrorCode::ConveyorTransitTooLong,
            belt: 0,
            site: FaultSite::Step { at_time: 1e9 },
            marker: Some((0, f64::NAN)),
            later_belt_marker: None,
            publish_marker: None,
            needs_flows: false,
            init_probe: None,
        },
    )
    .expect("wasm codegen with fault");

    let mut vm = Vm::new(compile(&datamodel)).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_level = vm.get_series(&crate::common::Ident::new("level")).unwrap();

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_eq!(get_error(store, inst), 0, "nothing raised");
        assert_eq!(saved_steps(store, inst), 6);

        let level_off = layout_offset(&artifact, "level");
        let n_slots = artifact.layout.n_slots;
        let slab = read_slab(store, inst, &artifact);
        for (row, expected) in vm_level.iter().enumerate() {
            assert_eq!(slab[row * n_slots + level_off], *expected, "row {row}");
        }
    });
}
