// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the conveyor belt pass lowering ([`super`]). Split out of `belt.rs`
//! to keep that file under the project line-count lint; this is the
//! `#[cfg(test)] mod tests` body, included via `#[path]` so `use super::*` still
//! resolves the module's private items.
//!
//! The bytecode VM is the oracle. Every parity test compiles the SAME datamodel
//! through `queue_compile::build_vm` (the VM's special-stock build path) and
//! through `module::compile_datamodel_including_conveyors` (which routes through
//! the same `queue_compile::compile_sim` dispatch, minus the public entry point's
//! not-yet-lifted conveyor reject), runs the blob under the DLR-FT interpreter, and
//! diffs the two slabs variable by variable.
//!
//! Where a belt's trajectory is simple enough to state in closed form, the test ALSO
//! pins the expected series independently -- a VM-vs-wasm diff alone would pass
//! vacuously if both backends were wrong the same way.

use crate::common::{Canonical, ErrorCode, Ident};
use crate::conveyor::SlatBoundGuard;
use crate::conveyor_compile::ConveyorPlan;
use crate::db::{SimlinDb, sync_from_datamodel_incremental};
use crate::queue_compile::{build_vm, compile_sim};
use crate::wasmgen::module::compile_datamodel_including_conveyors;
use crate::wasmgen::{WasmArtifact, WasmGenError, reconstruct_error};
use checked::{Store, Stored, StoredValue};
use std::io::BufReader;
use wasm::addrs::ModuleAddr;
use wasm::validate;

type TestStore<'a> = Store<'a, ()>;
type Inst = Stored<ModuleAddr>;

/// A VM-vs-wasm mismatch above this is a failure. Both backends run the identical
/// opcode program over the identical slab, and the belt pass is a transcription
/// whose accumulation orders were matched term for term -- so in practice every
/// assertion below holds at 0 ULP. The tolerance exists only to keep an unrelated
/// `f64::max(-0.0, 0.0)` sign ambiguity (which Rust documents as
/// non-deterministic, and which no test observes) from ever becoming a flake.
const EPS: f64 = 1e-9;

// ── drivers ──────────────────────────────────────────────────────────────────

fn parse(xml: &str) -> crate::datamodel::Project {
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("parse xmile")
}

fn artifact_for(project: &crate::datamodel::Project) -> WasmArtifact {
    let main = project.models[0].name.clone();
    compile_datamodel_including_conveyors(project, &main)
        .expect("a core conveyor model must lower to wasm")
}

fn lower_err(project: &crate::datamodel::Project) -> WasmGenError {
    let main = project.models[0].name.clone();
    match compile_datamodel_including_conveyors(project, &main) {
        Ok(_) => panic!("an out-of-scope conveyor feature must be rejected"),
        Err(e) => e,
    }
}

/// The conveyor plan list `queue_compile::compile_sim` resolves -- the SAME plans
/// the VM attaches, so `reconstruct_error` reads the belt names and slot offsets
/// the VM used.
fn conveyor_plans(project: &crate::datamodel::Project) -> Vec<ConveyorPlan> {
    let main = project.models[0].name.clone();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_sim(&mut db, sync.project, project, &main)
        .expect("conveyor model must build")
        .conveyor_plans
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

/// One f64 out of the live `curr` chunk (whose base is byte 0).
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

/// The whole live `curr` chunk, copied out so `reconstruct_error`'s slot reader can
/// borrow it immutably (the DLR-FT store's memory accessor wants `&mut store`).
fn read_curr(store: &mut TestStore<'_>, inst: Inst, artifact: &WasmArtifact) -> Vec<f64> {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
    store.mem_access_mut_slice(mem, |bytes| {
        (0..artifact.layout.n_slots)
            .map(|i| f64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap()))
            .collect()
    })
}

/// The blob's live saved-row counter (`saved_steps`), the wasm twin of the VM's
/// `Results::step_count`.
fn saved_steps(store: &mut TestStore<'_>, inst: Inst) -> usize {
    let g = store
        .instance_export(inst, "saved_steps")
        .expect("saved_steps export")
        .as_global()
        .expect("saved_steps is a global");
    match store.global_read(g) {
        StoredValue::I32(x) => x as usize,
        other => panic!("expected an i32 global, got {other:?}"),
    }
}

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

/// Linear memory's current byte length -- the observable the leak test watches.
fn memory_bytes(store: &mut TestStore<'_>, inst: Inst) -> usize {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
    store.mem_access_mut_slice(mem, |bytes| bytes.len())
}

fn run_artifact(artifact: &WasmArtifact) -> Vec<f64> {
    with_instance(artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_eq!(get_error(store, inst), 0, "the run must raise no error");
        read_slab(store, inst, artifact)
    })
}

fn vm_slab(project: &crate::datamodel::Project) -> crate::Results {
    let main = project.models[0].name.clone();
    let mut vm = build_vm(project, &main).expect("VM must build the conveyor model");
    vm.run_to_end().expect("VM must run the conveyor model");
    vm.into_results()
}

/// Assert every variable in `artifact.layout` matches the VM's series, NaN-aware.
/// Returns the number of variables compared.
fn assert_slab_matches_vm(project: &crate::datamodel::Project, artifact: &WasmArtifact) -> usize {
    let wasm_data = run_artifact(artifact);
    let vm = vm_slab(project);

    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    assert_eq!(vm.step_count, n_chunks, "saved-chunk count differs from VM");

    let mut checked = 0usize;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let v = vm.data[c * vm.step_size + vm_off];
            let w = wasm_data[c * n_slots + *wasm_off];
            // `v == w` first: it is the common case, and it is the ONLY way two
            // infinities compare equal here -- `(INF - INF).abs() < EPS` is
            // `NaN < EPS`, i.e. false, so a difference-only check would report a
            // spurious mismatch on any model whose flows go infinite.
            if v == w || (v.is_nan() && w.is_nan()) {
                continue;
            }
            assert!(
                (v - w).abs() < EPS,
                "{name} mismatch at chunk {c}: vm={v} wasm={w}"
            );
        }
        checked += 1;
    }
    checked
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

fn wasm_series(artifact: &WasmArtifact, slab: &[f64], name: &str) -> Vec<f64> {
    let off = layout_offset(artifact, name);
    (0..artifact.layout.n_chunks)
        .map(|c| slab[c * artifact.layout.n_slots + off])
        .collect()
}

/// A one-belt model: a constant `inflow` feeds a belt of transit `len`, whose
/// primary outflow drains into `sink`. `extra` splices further `<conveyor>` children
/// (`<capacity>`, `<in_limit>`) into the block.
fn one_belt_xmile(
    dt: &str,
    stop: &str,
    initial: &str,
    len: &str,
    inflow: &str,
    extra: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>{stop}</stop><dt>{dt}</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>{initial}</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>{len}</len>{extra}</conveyor></stock>
    <flow name="in_f"><eqn>{inflow}</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
    )
}

// ── the checked-in fixtures ──────────────────────────────────────────────────

/// `test/conveyors/minimal_conveyor.xmile`: a capacity-limited belt of 16 slats
/// (transit 4, dt 0.25) initialized to 1000 units. The whole-corpus parity gate is
/// GH #924's; this pins the fixture directly, today.
#[test]
fn minimal_conveyor_fixture_matches_vm() {
    let project = parse(include_str!(
        "../../../../test/conveyors/minimal_conveyor.xmile"
    ));
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 4, "expected the whole model, checked {checked}");

    // Independent oracle for the first four steps: 1000 units spread over 16 slats
    // is 62.5 per slat, and the exit slat discharges each DT at 62.5/0.25 = 250.
    // Capacity 1200 never binds (contents stay at 1000: in == out == 250).
    let slab = run_artifact(&artifact);
    let graduating = wasm_series(&artifact, &slab, "graduating");
    for (i, &v) in graduating.iter().take(8).enumerate() {
        assert!((v - 250.0).abs() < EPS, "graduating[{i}] = {v}, want 250");
    }
    let students = wasm_series(&artifact, &slab, "students");
    for (i, &v) in students.iter().take(8).enumerate() {
        assert!((v - 1000.0).abs() < EPS, "students[{i}] = {v}, want 1000");
    }
}

/// `test/conveyors/arrayed_conveyor.xmile`: an arrayed conveyor is N independent
/// belts (§10), flattened by `resolve_plans` into one `ConveyorPlan` per element --
/// so the pass emitter sees N scalar plans, each with its OWN transit time (element
/// `a` has 8 slats, `b` has 16).
#[test]
fn arrayed_conveyor_fixture_matches_vm() {
    let project = parse(include_str!(
        "../../../../test/conveyors/arrayed_conveyor.xmile"
    ));
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 6, "expected both elements, checked {checked}");

    // Each belt starts empty and fills for its own transit time before discharging:
    // element a (transit 2) first outputs at t=2, element b (transit 4) at t=4.
    let slab = run_artifact(&artifact);
    let out_a = wasm_series(&artifact, &slab, "outflow_f[a]");
    let out_b = wasm_series(&artifact, &slab, "outflow_f[b]");
    // dt = 0.25, so row 7 is t = 1.75 and row 8 is t = 2.0.
    assert_eq!(out_a[7], 0.0, "belt a must not discharge before t=2");
    assert!((out_a[8] - 100.0).abs() < EPS, "out_a[t=2] = {}", out_a[8]);
    assert_eq!(out_b[15], 0.0, "belt b must not discharge before t=4");
    assert!(
        (out_b[16] - 250.0).abs() < EPS,
        "out_b[t=4] = {}",
        out_b[16]
    );
}

// ── the belt's behavioral surface ────────────────────────────────────────────

/// An empty belt fed a constant rate delays that rate by exactly the transit time,
/// then passes it through. This is the whole point of a conveyor.
#[test]
fn single_belt_transport_matches_vm() {
    let project = parse(&one_belt_xmile("0.5", "6", "0", "2", "10", ""));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Four slats (2 / 0.5). Nothing exits until the first cohort has traveled the
    // whole belt: rows 0..3 (t = 0, 0.5, 1.0, 1.5) discharge 0, and row 4 (t = 2)
    // discharges the full 10.
    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    assert_eq!(&out[..4], &[0.0, 0.0, 0.0, 0.0], "out_f = {out:?}");
    for (i, &v) in out.iter().enumerate().skip(4) {
        assert!((v - 10.0).abs() < EPS, "out_f[{i}] = {v}, want 10");
    }
    // Conservation: the belt holds exactly one transit-time's worth of material.
    let belt = wasm_series(&artifact, &slab, "belt");
    for (i, &v) in belt.iter().enumerate().skip(4) {
        assert!((v - 20.0).abs() < EPS, "belt[{i}] = {v}, want 10*2");
    }
}

/// A transit time that is an exact multiple of dt, one that is not, and -- the case
/// that separates round-half-AWAY-from-zero from wasm's round-half-to-EVEN
/// `f64.nearest` -- one whose `transit/dt` is exactly `x.5`.
///
/// `1.25 / 0.5 == 2.5` exactly in binary, so `slat_count` must give 3 (half away
/// from zero), never 2. A three-slat belt first discharges at t = 1.5; a two-slat
/// one would discharge at t = 1.0.
#[test]
fn transit_dt_ratios_match_vm() {
    for (dt, len, first_out_row) in [
        ("0.25", "1.25", 5usize), // 5 slats exactly
        ("0.5", "1.4", 3),        // 2.8 -> 3 slats; effective transit 1.5
        ("0.5", "1.25", 3),       // 2.5 -> 3 slats (half AWAY from zero)
        ("0.5", "0.75", 2),       // 1.5 -> 2 slats
        ("0.5", "0.5", 1),        // exactly one slat
    ] {
        let project = parse(&one_belt_xmile(dt, "4", "0", len, "8", ""));
        let artifact = artifact_for(&project);
        assert_slab_matches_vm(&project, &artifact);

        let slab = run_artifact(&artifact);
        let out = wasm_series(&artifact, &slab, "out_f");
        assert!(
            out[first_out_row - 1] == 0.0,
            "dt={dt} len={len}: belt discharged early, out_f = {out:?}"
        );
        assert!(
            (out[first_out_row] - 8.0).abs() < EPS,
            "dt={dt} len={len}: out_f[{first_out_row}] = {}, want 8",
            out[first_out_row]
        );
    }
}

/// `<capacity>` bounds instantaneous contents (§6.3), crediting the room this DT's
/// outflow freed. A belt at capacity admits only what just left.
#[test]
fn capacity_limited_admission_matches_vm() {
    let project = parse(&one_belt_xmile(
        "0.5",
        "8",
        "0",
        "2",
        "40",
        "<capacity>30</capacity>",
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!(
        belt.iter().all(|v| *v <= 30.0 + EPS),
        "capacity 30 exceeded: {belt:?}"
    );
    // The admitted rate is clamped in place, so `in_f` reports the ADMITTED rate
    // (not the requested 40) once the belt is full.
    let in_f = wasm_series(&artifact, &slab, "in_f");
    assert!(
        in_f.iter().any(|v| (*v - 40.0).abs() > EPS),
        "capacity must bind somewhere: in_f = {in_f:?}"
    );
    assert!(
        in_f.iter().all(|v| *v <= 40.0 + EPS && *v >= -EPS),
        "admitted rate out of range: {in_f:?}"
    );
}

/// `<in_limit>` bounds equation-driven inflow per TIME UNIT; a CONTINUOUS conveyor
/// prorates it to `in_limit * dt` each DT (§6.3).
#[test]
fn inflow_limit_matches_vm() {
    let project = parse(&one_belt_xmile(
        "0.5",
        "6",
        "0",
        "1",
        "25",
        "<in_limit>10</in_limit>",
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Each DT admits at most `10 * 0.5 = 5` volume, i.e. a rate of 10.
    let slab = run_artifact(&artifact);
    let in_f = wasm_series(&artifact, &slab, "in_f");
    for (i, &v) in in_f.iter().enumerate() {
        assert!((v - 10.0).abs() < EPS, "in_f[{i}] = {v}, want the limit 10");
    }
}

/// A belt seeded with a positive initial value spreads it evenly across its slats
/// (§7.1's leak-free closed form, `c[i] = 1` so each slat holds `V/N`) and then
/// discharges that even fill one slat per DT.
#[test]
fn steady_initial_fill_matches_vm() {
    let project = parse(&one_belt_xmile("0.5", "4", "60", "1.5", "0", ""));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // 3 slats (1.5/0.5), so 20 per slat, discharged at 20/0.5 = 40 for three steps.
    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!((out[0] - 40.0).abs() < EPS, "out_f = {out:?}");
    assert!((out[1] - 40.0).abs() < EPS, "out_f = {out:?}");
    assert!((out[2] - 40.0).abs() < EPS, "out_f = {out:?}");
    assert_eq!(out[3], 0.0, "the belt is empty after three steps: {out:?}");
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!((belt[0] - 60.0).abs() < EPS, "belt[0] = {}", belt[0]);
}

/// A §7.2 explicit init list with one entry per SLAT fills the belt front first, so
/// the entries discharge in list order.
#[test]
fn explicit_init_list_per_slat_matches_vm() {
    // 4 slats (1 / 0.25) and a four-entry list.
    let project = parse(&one_belt_xmile("0.25", "2", "1, 2, 3, 4", "1", "0", ""));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    // The list fills slat j with entry j (0 = exit), so the outflow VOLUME sequence
    // is 1, 2, 3, 4 -- reported as a rate (volume / dt).
    for (i, want) in [1.0, 2.0, 3.0, 4.0].iter().enumerate() {
        assert!(
            (out[i] - want / 0.25).abs() < EPS,
            "out_f[{i}] = {}, want {}",
            out[i],
            want / 0.25
        );
    }
    // And the stock's initial is the list total, not the placeholder.
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!((belt[0] - 10.0).abs() < EPS, "belt[0] = {}", belt[0]);
}

/// A §7.2 list whose length is NOT the slat count is one entry per TIME UNIT: each
/// entry spreads evenly across its block's slats, so the outflow during unit `u`
/// totals `v_u`. A short list repeats its last entry.
#[test]
fn explicit_init_list_per_time_unit_matches_vm() {
    // 8 slats (2 / 0.25), U = floor(7*0.25) + 1 = 2 time-unit blocks: slats 0..3
    // (block 0) and 4..7 (block 1). A three-entry list truncates to two.
    let project = parse(&one_belt_xmile("0.25", "3", "40, 80, 999", "2", "0", ""));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!((belt[0] - 120.0).abs() < EPS, "belt[0] = {}", belt[0]);
    let out = wasm_series(&artifact, &slab, "out_f");
    // Block 0 holds 40 spread over four slats: 10 each, i.e. a rate of 40 for the
    // first four steps. Block 1 holds 80 over four slats: 20 each, rate 80.
    for (i, v) in out.iter().take(8).enumerate() {
        let want = if i < 4 { 40.0 } else { 80.0 };
        assert!((v - want).abs() < EPS, "out_f[{i}] = {v}");
    }
}

/// `INIT()` of a §7.2 list-initialized conveyor stock -- and any ordinary initial
/// reading it -- must see the NORMALIZED belt total, not the compiled placeholder.
/// The reconcile-skipping initials re-run is what supplies that.
#[test]
fn list_init_reconciles_dependent_initials_like_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>40, 80, 999</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"></flow>
    <aux name="start_total"><eqn>INIT(belt)</eqn></aux>
    <aux name="frac"><eqn>belt / INIT(belt)</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // The raw list sum is 40+80+999 = 1119; the NORMALIZED total (999 truncated) is
    // 120. A blob that froze the raw-sum placeholder would report 1119 here.
    for (i, &v) in wasm_series(&artifact, &slab, "start_total")
        .iter()
        .enumerate()
    {
        assert!(
            (v - 120.0).abs() < EPS,
            "step {i}: INIT(belt) = {v}, want 120"
        );
    }
    assert!(
        wasm_series(&artifact, &slab, "frac")
            .iter()
            .all(|v| v.is_finite()),
        "INIT(belt) must not be 0"
    );
}

/// Two belts in one model, the first feeding the second, whose outflow feeds an
/// ordinary stock. The upstream belt's outflow is a CONVEYOR-DRIVEN inflow of the
/// downstream one: admitted unconditionally, bypassing its capacity (§4.3 step 4).
///
/// This also pins the phase A / phase B split: the downstream belt reads the
/// upstream's driven rate, which only exists because phase A ran over BOTH belts
/// before either phase B did. An interleaved `phase_a(i); phase_b(i)` emission is
/// only observable when the DOWNSTREAM belt sorts first in plan order (it would then
/// admit the prior step's rate), so the case is run twice with the belts' names AND
/// declaration order swapped -- whichever of the two the plan order actually keys on,
/// one of the two runs puts the downstream belt at plan index 0.
#[test]
fn chained_belts_match_vm() {
    for (up, down) in [("belt_a", "belt_z"), ("belt_z", "belt_a")] {
        let up_block = format!(
            r#"<stock name="{up}"><eqn>0</eqn><inflow>in_f</inflow><outflow>mid_f</outflow>
      <conveyor><len>1</len></conveyor></stock>"#
        );
        let down_block = format!(
            r#"<stock name="{down}"><eqn>0</eqn><inflow>mid_f</inflow><outflow>out_f</outflow>
      <conveyor><len>1.5</len><capacity>1</capacity></conveyor></stock>"#
        );
        // Declare them in name order, so `up`/`down` swap covers both a
        // declaration-ordered and a name-ordered plan list.
        let (a, b) = if up < down {
            (&up_block, &down_block)
        } else {
            (&down_block, &up_block)
        };
        let project = parse(&format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    {a}
    {b}
    <flow name="in_f"><eqn>12</eqn></flow>
    <flow name="mid_f"></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
        ));
        let artifact = artifact_for(&project);
        let checked = assert_slab_matches_vm(&project, &artifact);
        assert!(checked >= 6, "expected both belts, checked {checked}");

        let slab = run_artifact(&artifact);
        // The chain delays by 1 + 1.5 = 2.5 time units -- five DTs. The downstream
        // belt's capacity of 1 is IGNORED for its conveyor-driven inflow, so the full
        // 12 arrives.
        let out = wasm_series(&artifact, &slab, "out_f");
        assert_eq!(&out[..5], &[0.0; 5], "upstream={up}: out_f = {out:?}");
        assert!(
            (out[5] - 12.0).abs() < EPS,
            "upstream={up}: out_f[5] = {}",
            out[5]
        );
        let down_s = wasm_series(&artifact, &slab, down);
        assert!(
            down_s.iter().any(|v| *v > 1.0 + EPS),
            "conveyor-driven inflow must bypass capacity: {down} = {down_s:?}"
        );
    }
}

/// A negative and a NaN requested inflow rate both clamp to 0 -- `rate.max(0.0)`
/// (`conveyor.rs:935`), whose Rust NaN semantics wasm's `f64.max` does NOT share.
/// This is the test that fails if `emit_clamp_nonneg`'s `select` is replaced by
/// `f64.max`.
#[test]
fn nonpositive_and_nan_inflows_clamp_like_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>swing</inflow><inflow>poison</inflow>
      <outflow>out_f</outflow><conveyor><len>1</len></conveyor></stock>
    <flow name="swing"><eqn>IF TIME MOD 2 = 0 THEN 6 ELSE -4</eqn></flow>
    <flow name="poison"><eqn>IF TIME &gt;= 2 THEN 0/0 ELSE 1</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let swing = wasm_series(&artifact, &slab, "swing");
    let poison = wasm_series(&artifact, &slab, "poison");
    assert!(
        swing.iter().all(|v| *v >= 0.0),
        "a negative inflow must be clamped in place: {swing:?}"
    );
    assert!(
        poison.iter().all(|v| !v.is_nan()),
        "a NaN inflow must clamp to 0 (Rust f64::max), not propagate: {poison:?}"
    );
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!(
        belt.iter().all(|v| v.is_finite()),
        "a NaN inflow must not poison the belt: {belt:?}"
    );
}

/// A time-varying `<len>` re-latches every DT (`<sample>` defaults to 1, §6.1) and
/// therefore grows and shrinks the belt mid-run (§6.2). Nothing in a `ConveyorPlan`
/// distinguishes a constant-valued `<len>` expression from this one, so the ring
/// must implement the general geometry -- which is what this test pins.
#[test]
fn time_varying_transit_matches_vm() {
    // Grows from 0.5 (1 slat) to 2.5 (5 slats), then shrinks back past its start.
    let project = parse(&one_belt_xmile(
        "0.5",
        "8",
        "0",
        "MAX(0.5, 2.5 - ABS(TIME - 4) * 0.5)",
        "10",
        "",
    ));
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 4, "checked {checked}");

    // A shrinking belt discharges shallower-inserted material EARLY (the documented
    // non-FIFO behavior), so the outflow is not simply the delayed inflow -- but the
    // total is conserved.
    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    let sink = wasm_series(&artifact, &slab, "sink");
    assert!(
        belt.iter().chain(sink.iter()).all(|v| v.is_finite()),
        "belt = {belt:?}, sink = {sink:?}"
    );
    assert!(
        belt.iter().any(|v| *v > 15.0),
        "the belt must fill as its transit grows: {belt:?}"
    );
}

/// `b_shrink` drops trailing EMPTY slats only. A shortened transit leaves the belt
/// over-long, and the material beyond the new entry depth must ride to the exit
/// rather than being discarded (§6.2, `conveyor.rs`'s `Some(s) if s.content == 0.0`
/// guard).
///
/// The entry depth drops 6 -> 2 in one DT at `t = 2`, which is the only way to enter
/// `b_shrink`'s loop at all: a one-slat drop is absorbed by the `pop_front` that
/// precedes it. Every tail slat is FULL at that moment, so a correct `b_shrink`
/// drops nothing. Dropping the `content == 0.0` guard silently destroys three slats
/// of material and the exit rate collapses from 20 to 10.
#[test]
fn shrink_stops_at_nonempty_tail() {
    let project = parse(&one_belt_xmile(
        "0.5",
        "5",
        "0",
        "IF TIME &lt; 2 THEN 3 ELSE 1",
        "10",
        "",
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    // Four cohorts of 5 entered while the belt was deep; after the collapse they
    // leave two-at-a-time from the merged entry slat, a rate of 20 rather than 10.
    assert!((out[7] - 20.0).abs() < EPS, "out_f[7] = {}", out[7]);
    // Conservation: nothing was thrown away when the belt shortened.
    let sink = wasm_series(&artifact, &slab, "sink");
    let belt = wasm_series(&artifact, &slab, "belt");
    let last = sink.len() - 1;
    assert!(
        (sink[last] + belt[last] - 10.0 * 0.5 * (last as f64)).abs() < EPS,
        "material was destroyed: sink={} belt={}",
        sink[last],
        belt[last]
    );
}

/// The complementary case to [`shrink_stops_at_nonempty_tail`]: `b_shrink`'s loop
/// BODY actually runs, dropping trailing empty slats.
///
/// An empty tail is reachable without leaks, contrary to the intuition that material
/// always enters at the back. Grow `<len>` while the inflow is 0 and `b_grow_to_d`
/// pushes empty slats behind the material with no cohort to fill them (`acc == 0.0`
/// skips the insert); dropping `<len>` afterwards leaves those empties past the entry
/// depth, and only then does `b_shrink` retire them.
///
/// Without this the whole loop body is dead in the test suite: a `b_shrink` that
/// stored `0` instead of `len - 1` -- annihilating the belt every time it fires --
/// would pass every other test.
#[test]
fn empty_tail_shrinks_without_leaks() {
    // Depth 7 -> 1 at t = 2; two cohorts of 5 enter before t = 1, then zeros.
    let project = parse(&one_belt_xmile(
        "0.5",
        "5",
        "0",
        "IF TIME &lt; 2 THEN 3.5 ELSE 0.5",
        "IF TIME &lt; 1 THEN 5 ELSE 0",
        "",
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // The two cohorts survive the collapse and reach the sink: shrink retired only
    // the empty slats behind them.
    let slab = run_artifact(&artifact);
    let sink = wasm_series(&artifact, &slab, "sink");
    let belt = wasm_series(&artifact, &slab, "belt");
    let last = sink.len() - 1;
    assert!(
        (sink[last] + belt[last] - 5.0).abs() < EPS,
        "the two cohorts total 5 volume: sink={} belt={}",
        sink[last],
        belt[last]
    );
}

/// `b_grow` copies the live slats out of the old ring IN RING ORDER, normalizing
/// `head` to 0. Every doubling reached by the other tests happens to start from
/// `head == 0`, where a linear `old[i] -> new[i]` copy is accidentally correct; here
/// four `pop_front`s have advanced `head` before the entry depth grows past the
/// capacity, so a head-ignoring copy rotates the belt and re-orders the exits.
///
/// The inflow ramps so every slat holds a distinct volume -- a uniform belt would
/// make any permutation of the ring invisible.
#[test]
fn ring_growth_with_nonzero_head() {
    let project = parse(&one_belt_xmile(
        "0.5",
        "6",
        "0",
        "IF TIME &lt; 1 THEN 2 ELSE 3.5",
        "TIME + 1",
        "",
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // The whole exit sequence, in closed form. The belt starts as four EMPTY slats
    // (initial 0), so the first four exits are 0. Then the `t = 0` and `t = 0.5`
    // cohorts leave (volume `(t + 1) * dt`, i.e. rate `t + 1`). At `t = 1` the depth
    // jumps 4 -> 7 and `b_grow_to_d` pushes three empty slats BEHIND the material and
    // ahead of the new entry slat, so three zeros exit before the ramp resumes.
    //
    // A head-ignoring ring copy rotates the live slats and replays a cohort out of
    // turn, which changes this sequence (and the belt contents) at the growth step.
    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    let want = [
        0.0, 0.0, 0.0, 0.0, 1.0, 1.5, 0.0, 0.0, 0.0, 2.0, 2.5, 3.0, 3.5,
    ];
    assert_eq!(out.len(), want.len(), "out_f = {out:?}");
    for (i, (&g, &w)) in out.iter().zip(want.iter()).enumerate() {
        assert!((g - w).abs() < EPS, "out_f[{i}] = {g}, want {w}");
    }
}

/// A conveyor-driven inflow is admitted unconditionally, but it still CHARGES the
/// capacity room the equation-driven inflows then compete for
/// (`ConveyorState::admission_room` subtracts `conv_vol`). This is the only belt
/// configuration where `rem_cap` is ever read: it needs a conveyor-driven inflow AND
/// an equation inflow AND a finite capacity, all on one belt.
///
/// Dropping `- conv_vol` lets the equation inflow admit the room the belt already
/// promised to the upstream belt, and the downstream belt overfills.
#[test]
fn conv_vol_charges_capacity_room() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="upstream"><eqn>12</eqn><inflow>in_f</inflow><outflow>mid_f</outflow>
      <conveyor><len>0.5</len></conveyor></stock>
    <flow name="in_f"><eqn>24</eqn></flow>
    <flow name="mid_f"></flow>
    <stock name="downstream"><eqn>0</eqn><inflow>mid_f</inflow><inflow>extra_f</inflow>
      <outflow>out_f</outflow>
      <conveyor><len>1.5</len><capacity>20</capacity></conveyor></stock>
    <flow name="extra_f"><eqn>100</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // The upstream belt starts FULL (one slat of 12), so at t = 0 it already delivers
    // `conv_vol = 12` into an EMPTY downstream belt of capacity 20. That is the step
    // where the two computations part company: the room left for `extra_f` is
    // `20 - 0 - 12 = 8` (a rate of `8 / dt = 16`), where a conv_vol-blind `cap_room`
    // would offer the full 20 and admit a rate of 40 -- overfilling the belt to 32.
    let extra = wasm_series(&artifact, &slab, "extra_f");
    assert!(
        (extra[0] - 16.0).abs() < EPS,
        "extra_f[0] = {}, want 16 (the conveyor-driven 12 charges the room)",
        extra[0]
    );
    // And the belt never exceeds the capacity its conveyor-driven inflow was already
    // promised: 12 + 8 lands exactly on 20.
    let down = wasm_series(&artifact, &slab, "downstream");
    assert!(
        (down[1] - 20.0).abs() < EPS,
        "downstream[1] = {}, want exactly the capacity 20",
        down[1]
    );
}

/// An INFINITE requested rate exhausts the clearance but must not POISON the inflows
/// listed after it. `rem_cap -= INF` would be `INF - INF = NaN`, and wasm's `f64.min`
/// propagates NaN (Rust's `f64::min` returns the non-NaN operand), so without the
/// `rem.is_finite()` select every later inflow clears NaN instead of its rate.
///
/// The assertion is on the SECOND inflow's column: the first one's is legitimately
/// infinite in both backends.
#[test]
fn infinite_rate_does_not_poison_later_inflows() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_a</inflow><inflow>in_b</inflow>
      <outflow>out_f</outflow><conveyor><len>1</len></conveyor></stock>
    <flow name="in_a"><eqn>1/0</eqn></flow>
    <flow name="in_b"><eqn>3</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let in_a = wasm_series(&artifact, &slab, "in_a");
    let in_b = wasm_series(&artifact, &slab, "in_b");
    assert!(
        in_a.iter().all(|v| v.is_infinite() && *v > 0.0),
        "in_a should stay +INF: {in_a:?}"
    );
    for (i, v) in in_b.iter().enumerate() {
        assert!(
            (v - 3.0).abs() < EPS,
            "in_b[{i}] = {v}: the infinite first inflow poisoned the clearance"
        );
    }
}

/// A §7.2 list SHORTER than the belt's time-unit block count repeats its last entry
/// (`norm(b) = table[min(b, m - 1)]`). The trailing comma is what makes `40,` a
/// one-entry list rather than a §7.1 scalar.
///
/// Two blocks, one entry: without the `min` clamp the second block indexes one f64
/// PAST the init-table data segment -- reading whatever the next region holds (zero
/// today), so the belt silently loses half its initial material.
#[test]
fn short_init_list_repeats_last_entry() {
    // 8 slats (2 / 0.25); U = floor(7 * 0.25) + 1 = 2 blocks of four slats each.
    let project = parse(&one_belt_xmile("0.25", "3", "40,", "2", "0", ""));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // Both blocks hold the repeated 40, spread over four slats: 10 each, rate 40.
    let out = wasm_series(&artifact, &slab, "out_f");
    for (i, v) in out.iter().take(8).enumerate() {
        assert!(
            (v - 40.0).abs() < EPS,
            "out_f[{i}] = {v}, want the repeated 40"
        );
    }
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!((belt[0] - 80.0).abs() < EPS, "belt[0] = {}", belt[0]);
}

// ── the runtime error channel, end to end ────────────────────────────────────

/// `init_belts` rejects a transit time that is not positive and finite. The blob
/// raises through the channel, saves no row, and the HOST rebuilds the VM's message
/// byte for byte.
#[test]
fn init_transit_not_positive_raises_like_vm() {
    // `<len>` is TIME, which is 0 at t = 0.
    let project = parse(&one_belt_xmile("0.25", "2", "0", "TIME", "10", ""));
    let main = project.models[0].name.clone();

    let mut vm = build_vm(&project, &main).expect("build");
    let vm_err = vm
        .run_to_end()
        .expect_err("a transit of 0 must be rejected");
    assert_eq!(vm_err.code, ErrorCode::ConveyorTransitNotPositive);
    let vm_message = vm_err.get_details().expect("message");

    let artifact = artifact_for(&project);
    let plans = conveyor_plans(&project);
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        let word = get_error(store, inst);
        let curr = read_curr(store, inst, &artifact);
        let (code, message) =
            reconstruct_error(word, &plans, 0.25, |off| curr[off]).expect("the blob must raise");
        assert_eq!(code, ErrorCode::ConveyorTransitNotPositive);
        assert_eq!(message, vm_message);
    });
}

/// A latched transit whose slat count exceeds the (test-shrunken) bound is rejected
/// at BELT INIT (§4.1), before the ring is allocated.
#[test]
fn init_transit_too_long_raises_like_vm() {
    let _guard = SlatBoundGuard::new(4);
    // 1.25 / 0.25 = 5 slats, one over the bound of 4.
    let project = parse(&one_belt_xmile("0.25", "2", "0", "1.25", "10", ""));
    let main = project.models[0].name.clone();

    let mut vm = build_vm(&project, &main).expect("build");
    let vm_err = vm.run_to_end().expect_err("5 slats against a bound of 4");
    assert_eq!(vm_err.code, ErrorCode::ConveyorTransitTooLong);
    let vm_message = vm_err.get_details().expect("message");

    let artifact = artifact_for(&project);
    let plans = conveyor_plans(&project);
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        let word = get_error(store, inst);
        let curr = read_curr(store, inst, &artifact);
        let (code, message) =
            reconstruct_error(word, &plans, 0.25, |off| curr[off]).expect("the blob must raise");
        assert_eq!(code, ErrorCode::ConveyorTransitTooLong);
        assert_eq!(message, vm_message);
        // Nothing was saved: `run_initials` returned before arming the cursor, so
        // `run_to`'s loop never ran a step.
        assert_eq!(saved_steps(store, inst), 0);
    });
}

/// The mid-run `<sample>` re-latch (implicit: `<sample>` defaults to 1) bound-checks
/// too. A `<len>` that grows past the bound raises from the step hook, and the blob
/// then saves no row for that step -- matching `vm.rs`'s `Err` between
/// `run_coupled_passes` and the Stocks phase.
#[test]
fn midrun_transit_too_long_raises_like_vm() {
    let _guard = SlatBoundGuard::new(4);
    // 0.25 + t*0.25: 1 slat at t=0, growing; slat_count crosses 4 at t = 1.0
    // (transit 0.5 -> 2 slats ... transit 1.25 at t=4 -> 5 slats).
    let project = parse(&one_belt_xmile(
        "0.25",
        "6",
        "0",
        "0.25 + TIME * 0.25",
        "10",
        "",
    ));
    let main = project.models[0].name.clone();

    let mut vm = build_vm(&project, &main).expect("build");
    let vm_err = vm
        .run_to_end()
        .expect_err("the growing belt must trip the bound");
    assert_eq!(vm_err.code, ErrorCode::ConveyorTransitTooLong);
    let vm_message = vm_err.get_details().expect("message");
    // The VM's `Results::step_count` is the slab CAPACITY, not the row count a
    // failed run wrote, so the expectation is derived from the model instead. With
    // `<len> = 0.25 + t*0.25` and dt = 0.25, `slat_count = floor(t + 1.5)`, which
    // first exceeds the bound of 4 at t = 3.5 -- the 15th step (t = 0 .. 3.25 are
    // the 14 that complete). The VM's own rows for those steps are the oracle below.
    let vm_data = vm.into_results();
    const COMPLETED: usize = 14;

    let artifact = artifact_for(&project);
    let plans = conveyor_plans(&project);
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        let word = get_error(store, inst);
        let curr = read_curr(store, inst, &artifact);
        let (code, message) =
            reconstruct_error(word, &plans, 0.25, |off| curr[off]).expect("the blob must raise");
        assert_eq!(code, ErrorCode::ConveyorTransitTooLong);
        assert_eq!(message, vm_message);
        assert_eq!(
            saved_steps(store, inst),
            COMPLETED,
            "the failing step must save no row"
        );

        // The rows that DID complete equal the VM's, so the failure did not corrupt
        // the run leading up to it.
        let slab = read_slab(store, inst, &artifact);
        let belt_wasm = layout_offset(&artifact, "belt");
        let belt_vm = vm_data.offsets[&Ident::<Canonical>::from_str_unchecked("belt")];
        for row in 0..COMPLETED {
            let v = vm_data.data[row * vm_data.step_size + belt_vm];
            let w = slab[row * artifact.layout.n_slots + belt_wasm];
            assert!((v - w).abs() < EPS, "row {row}: vm={v} wasm={w}");
        }
        // The failing step never wrote its row.
        assert_eq!(slab[COMPLETED * artifact.layout.n_slots + belt_wasm], 0.0);
    });

    // The channel is sticky: `reset` is the only way back.
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_ne!(get_error(store, inst), 0);
        call_void(store, inst, "run_initials");
        assert_ne!(get_error(store, inst), 0, "still set");
        call_void(store, inst, "reset");
        assert_eq!(get_error(store, inst), 0, "reset clears the channel");
    });
}

// ── lifecycle: preview, resume, reset ────────────────────────────────────────

/// A mid-run `get_value` must be side-effect-free. `run_to`'s tail re-publishes and
/// re-runs the pass as a PREVIEW on a cloned belt, so the resting `curr` holds the
/// pass-driven rate the resumed step will recompute -- and resuming still lands on
/// the byte-identical single-`run` slab, which it could not if the preview had
/// advanced the real belt.
///
/// The inflow RAMPS (`TIME`) so every slat holds a distinct volume. A uniform,
/// steady-state belt would make this test vacuous: shifting an all-equal ring is
/// the identity, so a preview that advanced the real belt -- or that never restored
/// the descriptor off its throwaway clone -- would leave no observable trace.
#[test]
fn midrun_preview_is_side_effect_free() {
    let project = parse(&one_belt_xmile("0.5", "6", "0", "1.5", "TIME", ""));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    // The VM's own mid-run preview (cloned side tables) is the oracle for the
    // resting `curr`, not a hand-derived constant.
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("VM must build the conveyor model");
    vm.run_to(3.0).expect("VM must run to the preview point");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 3.0);

        // Every variable's resting value -- the pass-driven outflow included, which a
        // bare Flows re-eval would stamp with its `0` placeholder.
        let mut checked = 0usize;
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(vm_off) = vm.get_offset(&ident) else {
                continue;
            };
            let (want, got) = (vm.get_value_now(vm_off), curr_slot(store, inst, *wasm_off));
            assert!(
                (want - got).abs() < EPS,
                "resting {name}: vm={want} wasm={got}"
            );
            checked += 1;
        }
        assert!(checked >= 4, "expected to compare the whole curr row");

        // Resuming to the end lands on the single-run slab: the preview left the
        // real belt untouched. A preview that advanced it would double-shift.
        call_run_to(store, inst, 6.0);
        assert_eq!(get_error(store, inst), 0);
        let resumed = read_slab(store, inst, &artifact);
        assert_eq!(single, resumed, "the preview double-advanced the belt");
    });
}

/// A RESUMED `run_to` must not rebuild the side table: `G_DID_INITIALS` short-circuits
/// `run_initials`, so the belt carries its slats across the segment boundary. The
/// proof is behavioral -- a re-init would refill the belt from the stock's initial.
#[test]
fn resumed_run_to_does_not_reinitialize_the_belt() {
    let project = parse(&one_belt_xmile("0.5", "6", "24", "1.5", "4", ""));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    let segmented = with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        for target in [1.5f64, 4.0, 6.0] {
            call_run_to(store, inst, target);
        }
        assert_eq!(get_error(store, inst), 0);
        read_slab(store, inst, &artifact)
    });
    assert_eq!(
        single, segmented,
        "a segmented run must land on the byte-identical slab"
    );
    assert_slab_matches_vm(&project, &artifact);
}

/// `reset` rewinds the shared bump pointer, so repeated `run`s (which delegate
/// `reset; run_to(stop)`) reproduce the identical slab AND settle on a fixed memory
/// footprint. A belt whose transit grows forces `b_grow`'s doubling; without the
/// rewind each run would leak the rings it abandoned.
#[test]
fn repeated_runs_are_identical_and_do_not_leak() {
    let project = parse(&one_belt_xmile(
        "0.25",
        "8",
        "0",
        "0.25 + TIME * 0.5",
        "3",
        "",
    ));
    let artifact = artifact_for(&project);
    with_instance(&artifact, |store, inst| {
        let mut prev_slab: Option<Vec<f64>> = None;
        let mut prev_bytes: Option<usize> = None;
        for round in 0..4 {
            call_void(store, inst, "run");
            assert_eq!(get_error(store, inst), 0);
            let slab = read_slab(store, inst, &artifact);
            let bytes = memory_bytes(store, inst);
            if let Some(p) = &prev_slab {
                assert_eq!(p, &slab, "run {round} diverged from the first run");
            }
            // Round 1 may still grow (the first run's doubling can cross a page);
            // from round 2 on the footprint must be stable.
            if let Some(p) = prev_bytes
                && round >= 2
            {
                assert_eq!(
                    p, bytes,
                    "run {round} grew linear memory: the bump pointer leaks across resets"
                );
            }
            prev_slab = Some(slab);
            prev_bytes = Some(bytes);
        }
    });
}

// ── set_value: pass-written slots are not overridable (GH #871) ──────────────

/// The blob's `set_value` must reject exactly the offsets the VM's
/// `set_value_by_offset` does. A belt's driven primary outflow compiles to a
/// placeholder `AssignConstCurr 0`, so the naive overridable-constant scan would
/// accept it -- and the pass overwrites it every step.
#[test]
fn set_value_rejects_the_driven_outflow() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>rate</eqn></flow>
    <flow name="out_f"></flow>
    <aux name="rate"><eqn>7</eqn></aux>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();
    let artifact = artifact_for(&project);
    let out_f = layout_offset(&artifact, "out_f");
    let rate = layout_offset(&artifact, "rate");

    with_instance(&artifact, |store, inst| {
        let set_value = store
            .instance_export(inst, "set_value")
            .expect("set_value export")
            .as_func()
            .expect("set_value is a function");
        let try_set = |store: &mut TestStore<'_>, off: usize, v: f64| -> i32 {
            store
                .invoke_simple_typed::<(i32, f64), i32>(set_value, (off as i32, v))
                .expect("set_value invoke")
        };
        assert_eq!(
            try_set(store, out_f, 1.0),
            1,
            "a driven outflow is not overridable"
        );
        assert_eq!(
            try_set(store, rate, 3.0),
            0,
            "a constant aux stays overridable"
        );
    });

    // And the VM agrees offset for offset.
    let mut vm = build_vm(&project, &main).expect("vm");
    assert!(vm.set_value_by_offset(out_f, 1.0).is_err());
    assert!(vm.set_value_by_offset(rate, 3.0).is_ok());
}

// ── what still rejects ───────────────────────────────────────────────────────

/// Every conveyor feature outside the step-1 core is refused LOUDLY on the internal
/// path, naming the feature -- never lowered as the simpler belt it is not.
#[test]
fn out_of_scope_conveyor_features_are_rejected() {
    let cases: [(&str, &str, &str); 5] = [
        (
            "leak flows",
            "leak",
            r#"<stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
                 <outflow>leak_f</outflow><conveyor><len>2</len></conveyor></stock>
               <flow name="in_f"><eqn>10</eqn></flow>
               <flow name="out_f"></flow>
               <flow name="leak_f"><eqn>0.1</eqn><leak/></flow>"#,
        ),
        (
            "discrete",
            "discrete",
            r#"<stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
                 <conveyor discrete="true"><len>2</len></conveyor></stock>
               <flow name="in_f"><eqn>10</eqn></flow>
               <flow name="out_f"></flow>"#,
        ),
        (
            "sample",
            "<sample>",
            r#"<stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
                 <conveyor><len>2</len><sample>1</sample></conveyor></stock>
               <flow name="in_f"><eqn>10</eqn></flow>
               <flow name="out_f"></flow>"#,
        ),
        (
            "arrest",
            "<arrest>",
            r#"<stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
                 <conveyor><len>2</len><arrest>0</arrest></conveyor></stock>
               <flow name="in_f"><eqn>10</eqn></flow>
               <flow name="out_f"></flow>"#,
        ),
        // Container access publishes at step start (§10, GH #923) and this pass
        // emits no publish hook. Were this arm ever dropped, `tot` would silently
        // read its placeholder 0 for the whole run rather than the belt's contents.
        (
            "container access",
            "container access",
            r#"<stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
                 <conveyor><len>2</len></conveyor></stock>
               <flow name="in_f"><eqn>10</eqn></flow>
               <flow name="out_f"></flow>
               <aux name="tot"><eqn>SUM(belt)</eqn></aux>"#,
        ),
    ];
    for (what, needle, body) in cases {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0"
       xmlns:isee="http://iseesystems.com/XMILE">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>{body}
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let Err(WasmGenError::Unsupported(msg)) =
            compile_datamodel_including_conveyors(&project, &main)
        else {
            panic!("{what} must be rejected, not silently mis-lowered");
        };
        assert!(
            msg.contains(needle),
            "{what}: the rejection must name the feature, got: {msg}"
        );
        assert!(
            msg.contains("not yet supported by the wasm backend"),
            "{what}: {msg}"
        );
    }
}

/// A `isee:spreadflow` inflow placement other than the default `beginning` is
/// rejected: `even`/`dest` need a per-slat spread and `dist`/`source` a per-step
/// weight vector, none of which this step lowers.
#[test]
fn spreadflow_placements_are_rejected() {
    // `even` and `dest` carry a non-`Beginning` placement; `dist` and `source` keep
    // `Beginning` and are caught only by their own disjuncts, so all four arms of
    // the spreadflow reject need a case.
    let cases = [
        ("even", r#"isee:spreadflow="even""#, ""),
        ("dest", r#"isee:spreadflow="dest""#, ""),
        (
            "dist",
            r#"isee:spreadflow="dist""#,
            "<isee:distrib_eq>1,2</isee:distrib_eq>",
        ),
        ("source", r#"isee:spreadflow="source""#, ""),
    ];
    for (what, attr, child) in cases {
        let project = parse(&format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0"
       xmlns:isee="http://iseesystems.com/XMILE">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f" {attr}><eqn>10</eqn>{child}</flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
        ));
        let WasmGenError::Unsupported(msg) = lower_err(&project);
        assert!(msg.contains("spreadflow"), "{what}: {msg}");
    }
}

/// `test/conveyors/covid19_severity.stmx` carries leak flows, so it rejects rather
/// than mis-lowering. GH #922 step 2 brings it under the parity harness.
///
/// (Its sibling `sir_social_distancing_mixnot.stmx` also holds a `dist` spread
/// inflow, but it never reaches this reject: its conveyor lives in a SUB-MODEL,
/// which `compile_sim` refuses for both backends -- so it is not a wasm-scope
/// fixture at all.)
#[test]
fn covid_leak_fixture_rejects() {
    let project = parse(include_str!(
        "../../../../test/conveyors/covid19_severity.stmx"
    ));
    let WasmGenError::Unsupported(msg) = lower_err(&project);
    assert!(msg.contains("leak"), "{msg}");
}

/// The PUBLIC entry point still rejects every conveyor model up front, whatever this
/// module can lower. GH #924 lifts it.
#[test]
fn public_entry_still_rejects_conveyor_models() {
    let project = parse(&one_belt_xmile("0.5", "2", "0", "1", "10", ""));
    let main = project.models[0].name.clone();
    let Err(WasmGenError::Unsupported(msg)) =
        crate::wasmgen::compile_datamodel_to_artifact(&project, &main, false, false)
    else {
        panic!("the public entry must still reject a conveyor model");
    };
    assert!(
        msg.contains("conveyor models are not yet supported"),
        "{msg}"
    );
}
