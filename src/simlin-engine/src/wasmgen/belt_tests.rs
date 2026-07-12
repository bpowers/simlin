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
//! through the PUBLIC `wasmgen::compile_datamodel_to_artifact` (which routes through
//! the same `queue_compile::compile_sim` dispatch), runs the blob under the DLR-FT
//! interpreter, and diffs the two slabs variable by variable.
//!
//! Where a belt's trajectory is simple enough to state in closed form, the test ALSO
//! pins the expected series independently -- a VM-vs-wasm diff alone would pass
//! vacuously if both backends were wrong the same way.

use crate::common::{Canonical, ErrorCode, Ident};
use crate::conveyor::SlatBoundGuard;
use crate::conveyor_compile::ConveyorPlan;
use crate::db::{SimlinDb, sync_from_datamodel_incremental};
use crate::queue_compile::{build_vm, compile_sim};
use crate::wasmgen::{
    WasmArtifact, WasmGenError, compile_datamodel_to_artifact, reconstruct_error,
};
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

/// Lower a conveyor model through the PUBLIC datamodel entry point. Since GH #924
/// there is no separate `#[cfg(test)]` seam: the entry every production wasm caller
/// uses is the entry these parity tests exercise.
fn lower(project: &crate::datamodel::Project) -> Result<WasmArtifact, WasmGenError> {
    let main = project.models[0].name.clone();
    compile_datamodel_to_artifact(project, &main, false, false)
}

fn artifact_for(project: &crate::datamodel::Project) -> WasmArtifact {
    lower(project).expect("a core conveyor model must lower to wasm")
}

fn lower_err(project: &crate::datamodel::Project) -> WasmGenError {
    match lower(project) {
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
            //
            // The bit-equal fast path is also where the belt pass's real FP guarantee
            // lives, so do not weaken it into a tolerance. Every leak fold is emitted
            // in the VM's own accumulation order (exit-first over slats, listed order
            // over flows, each seeded `+0.0`), which makes the two backends bit-identical
            // rather than merely close. Do NOT reason that a reordered fold is "safe
            // because it lands under EPS": `EPS` is an ABSOLUTE 1e-9, so on a belt
            // carrying 1e12 of material it is far tighter than one ULP, and on a belt
            // carrying 1e-12 it accepts a 100% error. The tolerance exists only to
            // absorb the transcendental helpers in `math.rs`; the belt does not need it.
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

/// One leak outflow of the [`leaky_belt_xmile`] belt.
#[derive(Clone, Copy)]
struct Leak {
    /// The leak fraction's `<eqn>`: a constant, or a `TIME`-varying expression.
    frac: &'static str,
    /// `leak_start`/`leak_end`, the zone's fractional positions from the ENTRY.
    /// `(0.0, 1.0)` is the whole belt, which is what a `<leak>` with no attributes
    /// means -- but the attributes are emitted anyway so the default and an explicit
    /// full zone are exercised by the same code path.
    zone: (f64, f64),
    /// `<leak_integers/>`.
    integers: bool,
}

impl Leak {
    const fn full(frac: &'static str) -> Leak {
        Leak {
            frac,
            zone: (0.0, 1.0),
            integers: false,
        }
    }
    const fn zoned(frac: &'static str, start: f64, end: f64) -> Leak {
        Leak {
            frac,
            zone: (start, end),
            integers: false,
        }
    }
    const fn integer(frac: &'static str) -> Leak {
        Leak {
            frac,
            zone: (0.0, 1.0),
            integers: true,
        }
    }
    const fn zoned_integer(frac: &'static str, start: f64, end: f64) -> Leak {
        Leak {
            frac,
            zone: (start, end),
            integers: true,
        }
    }
}

/// The knobs of [`leaky_belt_xmile`]. Every field is spelled at each call site rather
/// than defaulted: a test's hand-derived oracle depends on `dt`, `len`, and `inflow`
/// jointly, so a default would hide the very numbers its doc comment reasons about.
struct BeltXmile<'a> {
    dt: &'a str,
    stop: &'a str,
    /// The stock's `<eqn>`: a scalar volume, or the §7.2 explicit per-slat list.
    initial: &'a str,
    /// The `<len>` transit time. May be time-varying, which grows/shrinks the belt.
    len: &'a str,
    inflow: &'a str,
    /// Attributes on the `<conveyor>` element, e.g. `exponential_leak="true"`.
    conv_attrs: &'a str,
    /// Children of the `<conveyor>` element, e.g. `<capacity>`, `<in_limit>`.
    conv_extra: &'a str,
}

/// A one-belt model with leak flows. Each leak drains into its own `leak_sink_{k}`
/// stock, so the model reports every leaked volume as an integrable series and a test
/// can check conservation (`belt + sink + Σ leak_sink == Σ inflow`) rather than only
/// diffing against the VM.
fn leaky_belt_xmile(spec: BeltXmile<'_>, leaks: &[Leak]) -> String {
    let BeltXmile {
        dt,
        stop,
        initial,
        len,
        inflow,
        conv_attrs,
        conv_extra,
    } = spec;
    let outflow_tags: String = (0..leaks.len())
        .map(|k| format!("<outflow>leak_{k}</outflow>"))
        .collect();
    let leak_flows: String = leaks
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let integers = if l.integers { "<leak_integers/>" } else { "" };
            format!(
                r#"<flow name="leak_{k}" leak_start="{}" leak_end="{}"><eqn>{}</eqn><leak/>{integers}</flow>
                   <stock name="leak_sink_{k}"><eqn>0</eqn><inflow>leak_{k}</inflow></stock>"#,
                l.zone.0, l.zone.1, l.frac
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>{stop}</stop><dt>{dt}</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>{initial}</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      {outflow_tags}
      <conveyor {conv_attrs}><len>{len}</len>{conv_extra}</conveyor></stock>
    <flow name="in_f"><eqn>{inflow}</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
    {leak_flows}
  </variables></model>
</xmile>"#
    )
}

/// Flip a parsed project's conveyor stock to `ignore_earlier_zone_losses` (§5.1's
/// isee toggle). No XMILE spelling for it is confirmed, so the reader hard-codes
/// `false` (`xmile/variables.rs`) and the flag is reachable only from the datamodel --
/// which is exactly what both backends consume, so parity is still testable.
fn set_ignore_earlier_zone_losses(project: &mut crate::datamodel::Project) {
    let mut flipped = 0usize;
    for model in project.models.iter_mut() {
        for v in model.variables.iter_mut() {
            if let crate::datamodel::Variable::Stock(s) = v
                && let Some(c) = s.compat.conveyor.as_mut()
            {
                c.ignore_earlier_zone_losses = true;
                flipped += 1;
            }
        }
    }
    assert_eq!(flipped, 1, "expected exactly one conveyor stock to flip");
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

// ── leakage (§5) ─────────────────────────────────────────────────────────────

/// The whole-belt total this model conserves: `belt + sink + Σ leak_sink`.
fn conserved_total(artifact: &WasmArtifact, slab: &[f64], n_leaks: usize, row: usize) -> f64 {
    let at = |name: &str| wasm_series(artifact, slab, name)[row];
    (0..n_leaks)
        .map(|k| at(&format!("leak_sink_{k}")))
        .sum::<f64>()
        + at("belt")
        + at("sink")
}

/// A single linear leak over the whole belt: the fraction is of the material reaching
/// the zone, leaked evenly over the cohort's own `d`-slat journey (§5.1).
///
/// Independent oracle: 4 slats (2 / 0.5), a cohort of `A = 10 * 0.5 = 5` per DT, and
/// `f = 0.4`, so `basis = 0.4 * 5 / 4 = 0.5` leaves each slat each DT. At steady state
/// the belt holds `3.5 + 4 + 4.5 + 5 = 17`, the exit slat leaks BEFORE it discharges
/// (`3.5 - 0.5 = 3`, a rate of 6), and the four in-zone slats shed `4 * 0.5 = 2` per DT
/// (a rate of 4). `6 + 4 == 10`: exactly the inflow, and exactly `f * A` lost per
/// cohort over its lifetime.
#[test]
fn single_linear_leak_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.4")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    let leak = wasm_series(&artifact, &slab, "leak_0");
    let belt = wasm_series(&artifact, &slab, "belt");
    assert_eq!(&out[..4], &[0.0; 4], "nothing exits before t = 2: {out:?}");
    for row in 4..out.len() {
        assert!((out[row] - 6.0).abs() < EPS, "out_f[{row}] = {}", out[row]);
        assert!(
            (leak[row] - 4.0).abs() < EPS,
            "leak_0[{row}] = {}",
            leak[row]
        );
        assert!(
            (belt[row] - 17.0).abs() < EPS,
            "belt[{row}] = {}",
            belt[row]
        );
    }
    // The leak takes its cut from the exit slat before that slat discharges. Were the
    // order reversed the steady exit rate would be 7 and the belt would hold 16.
    assert!((out[4] - 6.0).abs() < EPS);
}

/// The §7.1 steady fill of a LEAKY belt is NOT the even spread the leak-free closed
/// form gives: it is the retained profile `c[i] = 1 - f*(N-1-i)/N`, scaled so the belt
/// totals `V`. Seeded at exactly its own equilibrium, the belt must stay there from
/// the first step -- no transient at all.
///
/// `N = 4`, `f = 0.4`: `c = [0.7, 0.8, 0.9, 1.0]`, `S = 3.4`, and `V = 17` gives
/// `E = 5`, i.e. slats `[3.5, 4, 4.5, 5]`. That is the same state
/// [`single_linear_leak_matches_vm`] reaches after four steps, so a UNIFORM init fill
/// (the leak-free closed form, `17/4 = 4.25` per slat) is loudly visible at row 0.
#[test]
fn leaky_steady_init_fill_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "17",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.4")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    let leak = wasm_series(&artifact, &slab, "leak_0");
    let belt = wasm_series(&artifact, &slab, "belt");
    for row in 0..out.len() {
        // A uniform fill would discharge (4.25 - 0.5) / 0.5 = 7.5 at row 0.
        assert!((out[row] - 6.0).abs() < EPS, "out_f[{row}] = {}", out[row]);
        assert!(
            (leak[row] - 4.0).abs() < EPS,
            "leak_0[{row}] = {}",
            leak[row]
        );
        assert!(
            (belt[row] - 17.0).abs() < EPS,
            "belt[{row}] = {}",
            belt[row]
        );
    }
}

/// Exponential leakage (§5.2): `f` is a per-time-unit RATE and each in-zone slat loses
/// `content * f * dt` from its start-of-step content, so the belt's steady profile is
/// geometric in `(1 - f*dt)` rather than linear.
///
/// `f = 0.6`, `dt = 0.5` ⇒ each slat keeps 70% per DT. A cohort crossing 4 slats leaves
/// with `0.7^4 = 0.2401` of what it entered with, so a constant inflow of 10 settles to
/// an outflow of `2.401` -- a number no linear-leak profile produces.
#[test]
fn exponential_leak_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: r#"exponential_leak="true""#,
            conv_extra: "",
        },
        &[Leak::full("0.6")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let out = wasm_series(&artifact, &slab, "out_f");
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert_eq!(&out[..4], &[0.0; 4], "nothing exits before t = 2: {out:?}");
    for row in 4..out.len() {
        assert!(
            (out[row] - 2.401).abs() < EPS,
            "out_f[{row}] = {}",
            out[row]
        );
        // Conservation at steady state: everything that enters, leaves.
        assert!(
            (out[row] + leak[row] - 10.0).abs() < EPS,
            "row {row}: out {} + leak {} != 10",
            out[row],
            leak[row]
        );
    }
}

/// §5.2's defining property: overlapping exponential rates ADD (they are all computed
/// from the same start-of-step content), so two 0.3/time flows behave exactly like one
/// 0.6/time flow and each reports half. Sequential compounding (`1 - 0.85*0.85`) would
/// give a different, larger, retained fraction.
#[test]
fn overlapping_exponential_rates_add() {
    let one = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: r#"exponential_leak="true""#,
            conv_extra: "",
        },
        &[Leak::full("0.6")],
    ));
    let two = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: r#"exponential_leak="true""#,
            conv_extra: "",
        },
        &[Leak::full("0.3"), Leak::full("0.3")],
    ));
    let (a1, a2) = (artifact_for(&one), artifact_for(&two));
    assert_slab_matches_vm(&one, &a1);
    assert_slab_matches_vm(&two, &a2);

    let (s1, s2) = (run_artifact(&a1), run_artifact(&a2));
    let out1 = wasm_series(&a1, &s1, "out_f");
    let out2 = wasm_series(&a2, &s2, "out_f");
    for (row, (&x, &y)) in out1.iter().zip(out2.iter()).enumerate() {
        assert!(
            (x - y).abs() < EPS,
            "row {row}: one-flow {x} != two-flow {y}"
        );
    }
    let l0 = wasm_series(&a2, &s2, "leak_0");
    let l1 = wasm_series(&a2, &s2, "leak_1");
    let total = wasm_series(&a1, &s1, "leak_0");
    for row in 0..l0.len() {
        assert!((l0[row] - l1[row]).abs() < EPS, "row {row}: flows disagree");
        assert!(
            (l0[row] + l1[row] - total[row]).abs() < EPS,
            "row {row}: the halves must sum to the single flow's rate"
        );
    }
}

/// §5.2's over-drain rule: when the summed exponential leaks would exceed a slat's
/// content, every flow scales down proportionally so exactly the content drains --
/// never negative, and still order-independent.
///
/// Two 1.5/time flows at `dt = 0.5` each want 75% of the slat; together 150%. Each must
/// take exactly half the content, and the belt must empty every step, so the inflow
/// passes straight into the two leaks and nothing ever reaches the exit.
#[test]
fn exponential_overdrain_scales_proportionally() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "0",
            len: "2",
            inflow: "8",
            conv_attrs: r#"exponential_leak="true""#,
            conv_extra: "",
        },
        &[Leak::full("1.5"), Leak::full("1.5")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let l0 = wasm_series(&artifact, &slab, "leak_0");
    let l1 = wasm_series(&artifact, &slab, "leak_1");
    let out = wasm_series(&artifact, &slab, "out_f");
    let belt = wasm_series(&artifact, &slab, "belt");
    for row in 1..out.len() {
        assert!(
            (l0[row] - l1[row]).abs() < EPS,
            "row {row}: not proportional"
        );
        assert!(out[row].abs() < EPS, "row {row}: out_f = {}", out[row]);
        // Only the just-inserted entry cohort survives a step (it enters after the
        // leak), so the belt holds exactly one DT of inflow: 8 * 0.5 = 4.
        assert!((belt[row] - 4.0).abs() < EPS, "belt[{row}] = {}", belt[row]);
        assert!(belt[row] >= 0.0, "a slat drained below zero");
    }
}

/// A PARTIAL leak zone (§5.3). Only the two entry-side slats of a 4-slat belt are in
/// zone, so the leak's magnitude depends on which slats those are -- and with a RAMPED
/// inflow every slat holds a different volume, making the choice observable.
///
/// Slat `i` (0 = exit) centers `1 - (i + 0.5)/4` from the entry: `0.875, 0.625, 0.375,
/// 0.125`. Zone `[0, 0.5]` therefore holds slats 2 and 3. Reading the position from the
/// EXIT instead (dropping the `1 -`) would select slats 0 and 1 -- a belt whose leak
/// still sums to `f * A` per cohort, and whose steady-state series a uniform inflow
/// could not tell apart.
#[test]
fn partial_entry_zone_leak_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "TIME + 1",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.5", 0.0, 0.5)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    // Row 0: the belt is empty, only the entry cohort is admitted (after the leak).
    assert_eq!(leak[0], 0.0, "an empty belt leaks nothing");
    // Row 1: exactly one cohort is on the belt, sitting at slat 3 (in zone). It entered
    // with `A = 1 * 0.5 = 0.5`, so `basis = A * r / M = 0.5 / 2 = 0.25` and it sheds
    // `f * basis = 0.125` -- a rate of 0.25. Over the two zone slats it will shed
    // `f * A = 0.25` in total, the whole documented fraction, even though the zone spans
    // only half the belt.
    assert!(
        (leak[1] - 0.125 / 0.5).abs() < EPS,
        "leak_0[1] = {}",
        leak[1]
    );
    // The zone spans two slats and every cohort crosses both, so each still loses
    // exactly `f = 0.5` of itself over its lifetime; conservation pins the rest.
    let last = leak.len() - 1;
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!(
        (conserved_total(&artifact, &slab, 1, last) - inflow).abs() < 1e-9,
        "material was destroyed"
    );
}

/// A leak zone that is neither anchored at the entry nor at the exit: a cohort ENTERS
/// the zone partway down the belt and LEAVES it before the exit. `[0.25, 0.75]` over a
/// 4-slat belt holds slats 1 and 2 (positions 0.625 and 0.375); slat 3 (0.125, the
/// entry) and slat 0 (0.875, the exit) are outside.
///
/// So the entry cohort rides one DT untouched, leaks for two, and rides one more before
/// discharging -- and the exit slat, being out of zone, discharges its FULL content.
#[test]
fn mid_belt_zone_leak_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "TIME + 1",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.5", 0.25, 0.75)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    // Rows 0 and 1: the only cohorts on the belt sit at slats 3 (row 1) / none (row 0),
    // both out of zone, so nothing has leaked yet.
    assert_eq!(leak[0], 0.0, "leak_0 = {leak:?}");
    assert_eq!(leak[1], 0.0, "the entry slat is outside [0.25, 0.75]");
    assert!(leak[2] > 0.0, "the zone must start biting at row 2");
    // The `t = 0` cohort (volume 0.5) exits at row 4 having crossed both zone slats:
    // it lost `f * A = 0.25`, so 0.25 remains -- a rate of 0.5.
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!((out[4] - 0.5).abs() < EPS, "out_f[4] = {}", out[4]);
}

/// STAGGERED zones (§5.1): with two non-identical zones, isee's default reads each `f`
/// as a fraction of the material REMAINING at the start of that flow's zone -- the
/// `r_k` factor. `ignore_earlier_zone_losses` selects the other reading, where each `f`
/// applies to the inflowing amount.
///
/// Leak 0 takes 0.4 over the entry half, leak 1 takes 0.5 over the exit half. Default:
/// leak 1 sees 0.6 of the cohort, so it removes 0.3 and 0.3 survives. With the toggle:
/// leak 1 removes 0.5 outright and 0.1 survives. Both must match the VM, and they must
/// NOT match each other -- if `r_k` were silently 1 (or silently computed), one of the
/// two would be wrong with no other symptom.
#[test]
fn ignore_earlier_zone_losses_changes_the_answer() {
    let xml = leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.4", 0.0, 0.5), Leak::zoned("0.5", 0.5, 1.0)],
    );
    let staggered = parse(&xml);
    let mut ignoring = parse(&xml);
    set_ignore_earlier_zone_losses(&mut ignoring);

    let (a_stag, a_ign) = (artifact_for(&staggered), artifact_for(&ignoring));
    assert_slab_matches_vm(&staggered, &a_stag);
    assert_slab_matches_vm(&ignoring, &a_ign);

    let (s_stag, s_ign) = (run_artifact(&a_stag), run_artifact(&a_ign));
    let out_stag = wasm_series(&a_stag, &s_stag, "out_f");
    let out_ign = wasm_series(&a_ign, &s_ign, "out_f");
    // A cohort of A = 5 arrives at the exit with 0.3*A = 1.5 (default) or 0.1*A = 0.5
    // (ignoring), i.e. a steady outflow rate of 3 or 1.
    for row in 4..out_stag.len() {
        assert!(
            (out_stag[row] - 3.0).abs() < EPS,
            "staggered out_f[{row}] = {}",
            out_stag[row]
        );
        assert!(
            (out_ign[row] - 1.0).abs() < EPS,
            "ignoring out_f[{row}] = {}",
            out_ign[row]
        );
    }
    // The two leaks are distinguishable too: `leak_1` sees the survivors by default.
    let l1_stag = wasm_series(&a_stag, &s_stag, "leak_1");
    let l1_ign = wasm_series(&a_ign, &s_ign, "leak_1");
    assert!((l1_stag[5] - 3.0).abs() < EPS, "leak_1 = {}", l1_stag[5]);
    assert!((l1_ign[5] - 5.0).abs() < EPS, "leak_1 = {}", l1_ign[5]);
}

/// Two linear leaks whose fractions sum ABOVE 1, both over the whole belt. §5.1 pins
/// isee's behavior: the flows leak in listed order and the per-slat content clamp means
/// "the last, or later, leakages may get less than their leak fraction suggests". With
/// identical (entry-anchored) zones `r_k = 1` for both, so it is the clamp, not the
/// retained profile, that resolves the conflict.
///
/// The clamp bites GRADUALLY, not by handing flow 0 its whole 0.8 up front: with
/// `N = 4` and `f = 0.8`, each flow wants `f * A / 4 = 0.2A` per in-zone DT. A cohort of
/// `A` therefore pays `0.2A` to each of the two flows on slats 3 and 2, is down to
/// `0.2A` at slat 1 where flow 0 takes the last of it and flow 1 gets nothing, and
/// arrives empty. Flow 0 ends with `0.6A`, flow 1 with `0.4A`, and the exit with
/// nothing. Swapping the listed order swaps the two totals -- and per row the earlier
/// flow can only ever take at least as much, never less.
#[test]
fn leak_fractions_summing_above_one_drain_in_listed_order() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "TIME + 1",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.8"), Leak::full("0.8")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let l0 = wasm_series(&artifact, &slab, "leak_0");
    let l1 = wasm_series(&artifact, &slab, "leak_1");
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!(
        out.iter().all(|v| v.abs() < EPS),
        "the two leaks take everything: out_f = {out:?}"
    );
    for row in 0..l0.len() {
        assert!(
            l0[row] >= l1[row] - EPS,
            "row {row}: the FIRST-listed flow never gets less, got {} vs {}",
            l0[row],
            l1[row]
        );
    }
    // Cumulatively the priority is unambiguous: 0.6A against 0.4A.
    let last = out.len() - 1;
    let sink = |k: usize| wasm_series(&artifact, &slab, &format!("leak_sink_{k}"))[last];
    assert!(
        sink(0) > sink(1) + 0.1,
        "flow 0 must out-take flow 1: {} vs {}",
        sink(0),
        sink(1)
    );
    // Conservation, including the two leak sinks.
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((conserved_total(&artifact, &slab, 2, last) - inflow).abs() < 1e-9);
}

/// A leak fraction above 1 clamps to 1, a negative one to 0, and a NaN one to 0 (§4.4's
/// `clamp_fraction`). The NaN arm is the one wasm cannot express with `f64.max`.
#[test]
fn leak_fraction_hygiene_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "5",
            initial: "0",
            len: "1",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full(
            "IF TIME &lt; 1 THEN 5 ELSE (IF TIME &lt; 2 THEN -3 ELSE 0/0)",
        )],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!(
        leak.iter().all(|v| !v.is_nan() && *v >= 0.0),
        "a NaN or negative fraction must clamp, not propagate: {leak:?}"
    );
    assert!(
        belt.iter().all(|v| v.is_finite() && *v >= -EPS),
        "belt = {belt:?}"
    );
    // t < 1: the fraction clamps to 1, so each cohort's whole basis leaks away over its
    // two-slat journey and nothing exits. t >= 2: the NaN clamps to 0, so the belt fills.
    let out = wasm_series(&artifact, &slab, "out_f");
    assert_eq!(&out[..2], &[0.0, 0.0], "out_f = {out:?}");
    assert!(out[out.len() - 1] > 0.0, "out_f = {out:?}");
}

/// `<leak_integers/>` (§5.4): the flow accumulates its real leak into a never-resetting
/// carry and removes only `floor(carry)` whole units, exit-most in-zone slat first.
///
/// The continuous rate here is `0.5 * (3 * 0.5) / 4 * 4 = 0.75` volume per DT once the
/// belt fills, so whole units come out in a 1-0-1-1-0... pattern rather than a steady
/// 1.5/DT rate -- and every reported rate is an integer over `dt`, i.e. a multiple of 2.
#[test]
fn leak_integers_carry_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "3",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::integer("0.5")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    for (row, &v) in leak.iter().enumerate() {
        let units = v * 0.5;
        assert!(
            (units - units.round()).abs() < EPS,
            "leak_0[{row}] = {v} is not a whole number of units per DT"
        );
    }
    assert!(
        leak.iter().any(|v| v.abs() < EPS),
        "some steps must leak nothing at all: {leak:?}"
    );
    assert!(
        leak.iter().any(|v| *v > 1.0),
        "and some must release a whole unit: {leak:?}"
    );
    // The carry conserves: nothing is created, and the fractional remainder stays on
    // the belt rather than being quietly dropped.
    let last = leak.len() - 1;
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!(
        (conserved_total(&artifact, &slab, 1, last) - inflow).abs() < 1e-9,
        "integer leakage must not create or destroy material"
    );
}

/// The integer carry is PER-FLOW and persists across the whole run, so a second integer
/// flow behind the first sees a different content (priority) and keeps its own carry.
/// This is the multi-integer-flow corner the spec leaves to simlin: the VM's undo /
/// requantize sequence runs once per flow in listed order, and the wasm must reproduce
/// the interleaving, not just the single-flow case.
#[test]
fn two_integer_leaks_keep_separate_carries() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "TIME + 2",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::integer("0.5"), Leak::integer("0.3")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    for k in 0..2 {
        let leak = wasm_series(&artifact, &slab, &format!("leak_{k}"));
        for (row, &v) in leak.iter().enumerate() {
            let units = v * 0.5;
            assert!(
                (units - units.round()).abs() < EPS,
                "leak_{k}[{row}] = {v} is not whole"
            );
        }
    }
    let last = wasm_series(&artifact, &slab, "out_f").len() - 1;
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((conserved_total(&artifact, &slab, 2, last) - inflow).abs() < 1e-9);
}

/// An INTEGER leak over a PARTIAL zone, which is what separates two otherwise-invisible
/// pieces of §5.4 from their no-op mutants.
///
/// Zone `[0, 0.2]` over a 4-slat belt holds only slat 3 (position 0.125), so `M = 1`,
/// `basis = A = 0.5`, and the continuous shed is `0.9 * 0.5 = 0.45` per DT.
///
/// 1. *Undelivered units return to the carry.* The carry crosses 1 at `t = 1.5`, but the
///    single in-zone slat holds only 0.5 -- half a unit. The flow delivers that 0.5 and
///    puts the other 0.5 BACK: without the return, the next step's carry sits at 0.35
///    instead of 0.85 and the leak simply skips a beat.
/// 2. *The `shed_by` row is per-step scratch.* A slat that was in zone last step has
///    moved out of it this step, and the requantization's undo pass adds `shed_by` back
///    for EVERY live slat. Leaving last step's value there hands the out-of-zone slat
///    0.45 of free material every step -- a leak that manufactures matter. A full-zone
///    integer leak (every other integer test here) can never see it, because a slat that
///    is always in zone always rewrites its row.
#[test]
fn integer_leak_over_a_partial_zone_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "1",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned_integer("0.9", 0.0, 0.2)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    // The carry is still below 1 for the first three steps, so nothing whole leaves.
    assert_eq!(&leak[..3], &[0.0; 3], "leak_0 = {leak:?}");
    // t = 1.5: `floor(1.35) = 1` unit is due but the zone holds 0.5, so 0.5 leaves.
    assert!((leak[3] - 0.5 / 0.5).abs() < EPS, "leak_0[3] = {}", leak[3]);
    // t = 2.0: the returned 0.5 keeps the carry above 1, so the flow fires again. A
    // dropped remainder would leave the carry at 0.80 and this step silent.
    assert!((leak[4] - 0.5 / 0.5).abs() < EPS, "leak_0[4] = {}", leak[4]);

    let last = leak.len() - 1;
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!(
        (conserved_total(&artifact, &slab, 1, last) - inflow).abs() < 1e-9,
        "a stale shed_by row would manufacture material"
    );
}

/// The §7.1 steady fill's `leak_basis` carries the `r_k` zone-start-retained factor, so a
/// staggered LATER-zone flow leaks against the material that survives the earlier zone
/// rather than against the original entry volume (§7.1 step 3).
///
/// [`ignore_earlier_zone_losses_changes_the_answer`] pins `r_k` on the *insert* path, but
/// an initially-empty belt scales every init schedule by `E = 0`, hiding the init path
/// entirely. Here the belt starts at its own equilibrium instead.
///
/// `N = 4`, leak 0 on `[0, 0.5]` (slats 3, 2) at `f = 0.4`, leak 1 on `[0.5, 1]`
/// (slats 1, 0) at `f = 0.5`. A unit cohort arrives at leak 1's zone holding `r_1 = 0.6`,
/// so `ub = [0.5, 0.3]` and the retained profile is `c = [0.45, 0.6, 0.8, 1.0]`,
/// `S = 2.85`. Seeding `V = 28.5` gives `E = 10` and slats `[4.5, 6, 8, 10]`, whose first
/// step sheds `4` to leak 0 and `3` to leak 1. Drop the `r_1` factor and `ub_1` becomes
/// 0.5, moving every one of those numbers.
#[test]
fn leaky_staggered_init_fill_uses_the_retained_factor() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "2",
            initial: "28.5",
            len: "2",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.4", 0.0, 0.5), Leak::zoned("0.5", 0.5, 1.0)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    assert!(
        (wasm_series(&artifact, &slab, "belt")[0] - 28.5).abs() < EPS,
        "the fill must total V whatever the profile"
    );
    let l0 = wasm_series(&artifact, &slab, "leak_0");
    let l1 = wasm_series(&artifact, &slab, "leak_1");
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!((l0[0] - 4.0 / 0.5).abs() < EPS, "leak_0[0] = {}", l0[0]);
    assert!((l1[0] - 3.0 / 0.5).abs() < EPS, "leak_1[0] = {}", l1[0]);
    // The exit slat holds 4.5, sheds 1.5 to leak 1, and discharges the remaining 3.
    assert!((out[0] - 3.0 / 0.5).abs() < EPS, "out_f[0] = {}", out[0]);
}

/// `<capacity>` room credits the room this DT's LEAK freed as well as its outflow
/// (`admission_room`'s `contents_after = c0 - leaked - out_vol`). A belt sitting at
/// capacity with a leak therefore admits strictly more than a leak-free one would.
#[test]
fn capacity_room_credits_the_leak() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "40",
            conv_attrs: "",
            conv_extra: "<capacity>20</capacity>",
        },
        &[Leak::full("0.5")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    let in_f = wasm_series(&artifact, &slab, "in_f");
    let leak = wasm_series(&artifact, &slab, "leak_0");
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!(
        belt.iter().all(|v| *v <= 20.0 + EPS),
        "capacity 20 exceeded: {belt:?}"
    );
    // Once the belt saturates, admission exactly replaces what left: outflow + leak.
    let last = belt.len() - 1;
    assert!(
        (in_f[last] - out[last] - leak[last]).abs() < EPS,
        "admitted {} != out {} + leak {}",
        in_f[last],
        out[last],
        leak[last]
    );
    // And the leak is a real part of that: a `contents_after` blind to it would offer
    // less room, so admission would be strictly smaller.
    assert!(leak[last] > EPS, "the leak must be live: {leak:?}");
}

/// A leaky belt whose `<len>` grows and shrinks mid-run (`<sample>` defaults to 1).
/// Zone membership is re-evaluated against the belt as it exists at that moment, so a
/// growing belt moves the zone boundaries under cohorts whose SCHEDULE was fixed at
/// insertion -- the corner §5.1's travel window exists to bound.
///
/// `b_grow_to_d` also pushes EMPTY slats behind the material; with a partial entry zone
/// those empties are in zone and must leak nothing (their basis and window are zero),
/// while the material behind them keeps its own budget.
#[test]
fn leak_with_growing_and_shrinking_belt_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "MAX(0.5, 2.5 - ABS(TIME - 4) * 0.5)",
            inflow: "TIME + 1",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.5", 0.0, 0.6)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert!(
        belt.iter().all(|v| v.is_finite() && *v >= -EPS),
        "belt = {belt:?}"
    );
    assert!(leak.iter().any(|v| *v > 0.0), "the leak never fired");
    // A cohort can only under-leak when the belt shifts under it (§5.1's window bound),
    // never over-leak: material is conserved.
    let last = belt.len() - 1;
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!(
        (conserved_total(&artifact, &slab, 1, last) - inflow).abs() < 1e-9,
        "material was destroyed"
    );
}

/// `b_shrink` retires trailing EMPTY slats. Step 1 could only ever reach that loop with
/// the empties `b_grow_to_d` pushes behind live material; a leak can EMPTY a slat that
/// really held something, so this exercises the same loop over leak-zeroed slats -- and
/// pins that a shrink still stops at the first non-empty tail slat.
///
/// A narrow ENTRY zone with a fraction of 1 is what drains a cohort outright: with
/// `[0, 0.3]` over a 7-slat belt only slats 6 and 5 are in zone, so `M = 2` and a cohort
/// sheds `A/2` on each of them -- arriving at slat 4 with exactly nothing, and riding
/// five more slats as a zero. The depth then collapses 7 -> 1 and `b_shrink` walks that
/// tail.
#[test]
fn leak_emptied_interior_slats_shrink() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "IF TIME &lt; 2 THEN 3.5 ELSE 0.5",
            inflow: "IF TIME &lt; 1 THEN 6 ELSE 0",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("1", 0.0, 0.3)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    let out = wasm_series(&artifact, &slab, "out_f");
    let last = belt.len() - 1;
    // The belt genuinely carried material before the leak zeroed it.
    assert!(belt.iter().any(|v| *v > 1.0), "belt = {belt:?}");
    // Two cohorts of 3 entered; the entry zone took all of both, so the exit never fires
    // and the belt is empty well before it collapses.
    assert!(
        out.iter().all(|v| v.abs() < EPS),
        "the entry zone takes everything: out_f = {out:?}"
    );
    assert!(belt[last].abs() < 1e-9, "belt[{last}] = {}", belt[last]);
    let leak_sink = wasm_series(&artifact, &slab, "leak_sink_0");
    assert!(
        (leak_sink[last] - 6.0).abs() < 1e-9,
        "leak_sink_0 = {}",
        leak_sink[last]
    );
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((conserved_total(&artifact, &slab, 1, last) - inflow).abs() < 1e-9);
}

/// A §7.2 explicit init list on a LEAKY belt. The list sets the CONTENTS; `fill_slats`
/// then hangs on each slat the schedule of an entry cohort that traveled there, scaled
/// to that slat's own content -- so slat `j` carries `window = basis * (j + 1)` in-zone
/// slats' worth of budget, not a uniform one.
///
/// The four entries are distinct, so a schedule derived from the wrong slat's content
/// (or a uniform basis) changes the leak series immediately.
#[test]
fn leaky_explicit_init_list_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.25",
            stop: "2",
            initial: "1, 2, 3, 4",
            len: "1",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.5")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!((belt[0] - 10.0).abs() < EPS, "belt[0] = {}", belt[0]);
    // Slat j (0 = exit) holds `j+1`, with `ub = r/M = 1/4`, so `basis_j = (j+1)/4` and
    // it sheds `0.5 * basis_j` on the first step: 0.125 + 0.25 + 0.375 + 0.5 = 1.25.
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert!(
        (leak[0] - 1.25 / 0.25).abs() < EPS,
        "leak_0[0] = {}",
        leak[0]
    );
    // The exit slat leaks first, then discharges what remains: 1 - 0.125 = 0.875.
    let out = wasm_series(&artifact, &slab, "out_f");
    assert!((out[0] - 0.875 / 0.25).abs() < EPS, "out_f[0] = {}", out[0]);
}

/// A time-varying leak fraction: the SCHEDULE (`basis`, `window`) is fixed when a
/// cohort enters, but the fraction is re-read every DT (§5.1's two sampling times). A
/// cohort that enters while the fraction is 0 must still leak once the fraction rises,
/// because its basis was never zero.
#[test]
fn time_varying_leak_fraction_matches_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("IF TIME &lt; 2 THEN 0 ELSE 0.4")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    // Nothing leaks while the fraction is 0, even though the belt fills.
    assert!(leak[..4].iter().all(|v| v.abs() < EPS), "leak_0 = {leak:?}");
    // The cohorts already on the belt then leak at the CURRENT fraction against the
    // basis they were given at insertion. A schedule frozen at the entry-time fraction
    // (0) would leak nothing, forever.
    assert!(leak[5] > EPS, "leak_0 = {leak:?}");
}

/// The mid-run preview must roll back the `<leak_integers/>` carry, which lives in a
/// static region rather than in the cloned ring. Nothing else in the belt's state has
/// that shape, so a preview that saves only the descriptor and the ring looks correct
/// on every other test.
///
/// A segmented run's previews each advance the carry by one step's worth of fractional
/// units; without the rollback the resumed steps quantize against a carry that ran
/// ahead, and the slab diverges from the single-`run` one.
#[test]
fn midrun_preview_rolls_back_the_integer_leak_carry() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "8",
            initial: "0",
            len: "2",
            inflow: "TIME + 3",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::integer("0.5")],
    ));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    let segmented = with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        for target in [1.5f64, 3.0, 5.5, 8.0] {
            call_run_to(store, inst, target);
        }
        assert_eq!(get_error(store, inst), 0);
        read_slab(store, inst, &artifact)
    });
    assert_eq!(
        single, segmented,
        "the preview leaked the integer carry into the real run"
    );

    // The segmented run's resting `curr` must also match the VM's own cloned-side-table
    // preview, leak rates included.
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("vm");
    vm.run_to(3.0).expect("vm run_to");
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 3.0);
        let ident = Ident::<Canonical>::from_str_unchecked("leak_0");
        let off = vm.get_offset(&ident).expect("leak_0 offset");
        let want = vm.get_value_now(off);
        let got = curr_slot(store, inst, layout_offset(&artifact, "leak_0"));
        assert!((want - got).abs() < EPS, "resting leak_0: {want} vs {got}");
    });
}

/// A leak flow feeding a DOWNSTREAM belt is a conveyor-driven inflow: admitted
/// unconditionally, bypassing that belt's capacity, and carrying the upstream's phase-A
/// leak volume. This is the case that pins the phase A / phase B split for LEAK rates,
/// not just for the primary outflow -- an interleaved emission would feed the
/// downstream belt the previous step's leak.
#[test]
fn leak_feeding_a_downstream_belt_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt_z"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_z</outflow>
      <outflow>spill</outflow><conveyor><len>1</len></conveyor></stock>
    <stock name="belt_a"><eqn>0</eqn><inflow>spill</inflow><outflow>out_a</outflow>
      <conveyor><len>1.5</len><capacity>1</capacity></conveyor></stock>
    <flow name="in_f"><eqn>TIME + 4</eqn></flow>
    <flow name="spill"><eqn>0.5</eqn><leak/></flow>
    <flow name="out_z"></flow>
    <flow name="out_a"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_z</inflow><inflow>out_a</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 6, "checked {checked}");

    let slab = run_artifact(&artifact);
    let down = wasm_series(&artifact, &slab, "belt_a");
    assert!(
        down.iter().any(|v| *v > 1.0 + EPS),
        "a leak-driven inflow bypasses capacity: {down:?}"
    );
    // Nothing is lost: the upstream belt sheds half of every cohort into the downstream
    // one, and both eventually drain into the sink.
    let last = down.len() - 1;
    let at = |n: &str| wasm_series(&artifact, &slab, n)[last];
    let inflow: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!(
        (at("belt_z") + at("belt_a") + at("sink") - inflow).abs() < 1e-9,
        "material was destroyed"
    );
}

/// A zone narrower than one slat at this DT resolution holds NO slat: `M_k(d) = 0`, so
/// the cohort leaks nothing to that flow (§5.1's last sentence) and the belt behaves as
/// if the flow were absent. This is the only test in which `b_first_zone` returns -1 and
/// the `M_k > 0` guards in `b_retained_{i}` / `b_insert_{i}` / the unit-basis
/// computation take their false arm -- without it, a `0 / 0` unit basis would silently
/// poison every schedule with NaN.
///
/// Slat centers on a 4-slat belt sit at `0.125, 0.375, 0.625, 0.875` from the entry;
/// `[0.4, 0.45]` contains none of them.
#[test]
fn zone_narrower_than_a_slat_leaks_nothing() {
    let leaky = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "5",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.9", 0.4, 0.45)],
    ));
    let artifact = artifact_for(&leaky);
    assert_slab_matches_vm(&leaky, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert!(
        leak.iter().all(|v| *v == 0.0),
        "an empty zone must leak nothing, not NaN: {leak:?}"
    );
    // And the belt transports exactly as a leak-free one would.
    let plain = parse(&one_belt_xmile("0.5", "5", "0", "2", "10", ""));
    let plain_artifact = artifact_for(&plain);
    let plain_slab = run_artifact(&plain_artifact);
    let a = wasm_series(&artifact, &slab, "out_f");
    let b = wasm_series(&plain_artifact, &plain_slab, "out_f");
    assert_eq!(a, b, "an empty zone must not perturb transport");
}

/// §5.3's zone test is `zs <= pos <= ze` on BOTH ends, exactly, with no epsilon: a slat
/// sitting precisely on a zone edge is in the zone (`conveyor.rs:394`). Nothing else
/// pins the inclusivity, because in every other fixture the slat centers fall strictly
/// inside their zone, so `b_in_zone`'s two `F64Le`s could each be `F64Lt` and no test
/// would notice.
///
/// Here both ends are exact. Transit 1 at DT 0.5 gives a 2-slat belt whose centers sit
/// at `pos = 1 - 0.5/2 = 0.75` (the exit slat) and `pos = 1 - 1.5/2 = 0.25` (the entry
/// slat). The zone `[0.25, 0.75]` lands EXACTLY on both. With `<=` the belt leaks from
/// every slat; flip either comparison to `<` and that slat leaves the zone, `M_k(d)`
/// drops, and -- because BOTH centers are edges -- the belt stops leaking altogether.
/// So the `any(> 0)` guard below is not redundant with the VM diff: it states the
/// property in the direction the mutant breaks, and would fail even if some future
/// change made both backends silently agree on leaking nothing.
#[test]
fn zone_edge_exactly_on_slat_centers_is_inclusive() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "0",
            len: "1",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::zoned("0.5", 0.25, 0.75)],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert!(
        leak.iter().any(|v| *v > 0.0),
        "both slat centers sit exactly on a zone edge, so an inclusive `<=` must leak \
         from them: {leak:?}"
    );
}

/// The retained walk (§5.1's staggered-zone factor `r_k`) clamps its running cohort at
/// zero: `c = max(c - shed, 0)`. Two clamps implement it -- one in `b_retained_{i}`
/// (mirroring `conveyor.rs:445`) and one in the §7.1 fill's first pass (mirroring
/// `conveyor.rs:487`) -- and neither is exercised by a fixture whose leaks are gentle
/// enough that the unit cohort survives the walk.
///
/// This one drains it below zero on purpose. `pos` measures distance ALREADY TRAVELLED
/// from the entry, so the walk meets low-`pos` zones first: the two heavy leaks go in
/// the entry half `[0, 0.5]` and the victim in the exit half `[0.5, 1.0]`. On a 4-slat
/// belt (transit 2, DT 0.5) the centers sit at `0.125, 0.375, 0.625, 0.875`, so each
/// zone holds exactly 2 slats and `M_k = 2` throughout.
///
/// The walk starts at the entry with `c = 1`. Across each of the two entry-half slats
/// the pair sheds `0.9 * 1 / 2` apiece, i.e. `0.9` per slat, so an UNCLAMPED walk leaves
/// `c = 1 - 1.8 = -0.8` and hands the third leak `r_2 = -0.8`. That makes
/// `basis_2 = A * r_2 / M_2` negative, and leak 2 runs BACKWARDS -- it ADDS material to
/// the belt. Clamped, `r_2 = 0` and leak 2 correctly shuts off: everything reaching its
/// zone has already been taken. The `all(>= 0)` guard is what catches the mutant; a bare
/// VM diff catches it too, but only because the VM happens to clamp, so the guard states
/// the physical invariant instead of merely pinning agreement.
///
/// `initial: "100"` is load-bearing, not decoration: a zero-initial belt never runs the
/// §7.1 fill's own descending walk, so it leaves the SECOND clamp (belt.rs's `IL_TMP`
/// site) untested. Seeding a non-empty belt routes the same negative cohort through it.
#[test]
fn retained_walk_clamps_a_cohort_drained_below_zero() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "100",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[
            Leak::zoned("0.9", 0.0, 0.5),
            Leak::zoned("0.9", 0.0, 0.5),
            Leak::zoned("0.5", 0.5, 1.0),
        ],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let leak_2 = wasm_series(&artifact, &slab, "leak_2");
    assert!(
        leak_2.iter().all(|v| *v >= 0.0),
        "an unclamped retained factor turns the third leak into an inflow: {leak_2:?}"
    );
    // Guard against a degenerate setup: the two entry-half leaks must actually be
    // draining, or `r_2` never goes negative and the clamps are untested after all.
    let leak_0 = wasm_series(&artifact, &slab, "leak_0");
    assert!(
        leak_0.iter().any(|v| *v > 0.0),
        "the entry-half leaks must drain the cohort: {leak_0:?}"
    );
}

/// Non-finite belt state, end to end. `f64::min` RETURNS THE OTHER OPERAND when one is
/// NaN; wasm's `f64.min` propagates. §5.1 calls `f64::min` twice per (slat, flow), so
/// the moment a belt carries an infinity the two semantics part company -- and a
/// conveyor stock whose `<eqn>` is `1/0` is all it takes.
///
/// The steady fill scales every slat by `E = INF`, the first leak step computes
/// `INF - INF = NaN` contents, and the SECOND one asks for `min(leak_basis = INF,
/// leak_window = NaN)`. Rust answers `INF` -- so the VM reports an infinite leak rate --
/// while `f64.min` answers `NaN`. Both backends must report the same column, whatever it
/// is; this test fails loudly if `b_fmin` is replaced by `f64.min`.
#[test]
fn infinite_belt_contents_leak_like_vm() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "3",
            initial: "1/0",
            len: "2",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.5")],
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Pin the shape rather than trusting the diff alone: the first step's leak is
    // infinite (an infinite belt sheds an infinite volume), and the belt is NaN after.
    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    assert!(leak[0].is_infinite() && leak[0] > 0.0, "leak_0 = {leak:?}");
    assert!(
        leak[1].is_infinite() && leak[1] > 0.0,
        "the second step's `min(INF, NaN)` must still be INF: {leak:?}"
    );
    assert!(
        wasm_series(&artifact, &slab, "belt")[1].is_nan(),
        "INF - INF must leave the belt NaN"
    );
}

/// The `<leak_integers/>` carry is zeroed by belt init, so a second `run` (which is
/// `reset` then `run_to`) reproduces the first run's slab exactly. It lives in a static
/// region the bump-pointer rewind does not touch, so nothing else would clear it.
#[test]
fn reset_rezeroes_the_integer_leak_carry() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "2",
            inflow: "3",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::integer("0.5")],
    ));
    let artifact = artifact_for(&project);
    with_instance(&artifact, |store, inst| {
        let mut prev: Option<Vec<f64>> = None;
        for round in 0..3 {
            call_void(store, inst, "run");
            assert_eq!(get_error(store, inst), 0);
            let slab = read_slab(store, inst, &artifact);
            if let Some(p) = &prev {
                assert_eq!(p, &slab, "run {round} inherited the previous run's carry");
            }
            prev = Some(slab);
        }
    });
}

/// A leak flow's slot is pass-written: its placeholder `0` equation compiles to an
/// `AssignConstCurr` the overridable-constant scan sees, and the pass overwrites it
/// every step (GH #871). `set_value` must reject it, exactly as the VM does.
#[test]
fn set_value_rejects_the_driven_leak_flow() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.4")],
    ));
    let main = project.models[0].name.clone();
    let artifact = artifact_for(&project);
    let leak_0 = layout_offset(&artifact, "leak_0");

    with_instance(&artifact, |store, inst| {
        let set_value = store
            .instance_export(inst, "set_value")
            .expect("set_value export")
            .as_func()
            .expect("set_value is a function");
        assert_eq!(
            store
                .invoke_simple_typed::<(i32, f64), i32>(set_value, (leak_0 as i32, 1.0))
                .expect("set_value invoke"),
            1,
            "a driven leak flow is not overridable"
        );
    });

    let mut vm = build_vm(&project, &main).expect("vm");
    assert!(vm.set_value_by_offset(leak_0, 1.0).is_err());
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

// ── the hard flags: <sample>, <arrest>, the held exit, discrete belts (§6) ───

/// The knobs of [`flag_belt_xmile`]. Spelled at every call site for the reason
/// [`BeltXmile`] gives, except through [`FlagBelt::plain`], which names the
/// flag-free baseline a test then overrides one field of.
struct FlagBelt<'a> {
    dt: &'a str,
    stop: &'a str,
    initial: &'a str,
    len: &'a str,
    inflow: &'a str,
    /// `discrete="true"` (§6.4): whole-unit admission with a fractional carry, a
    /// per-TIME-UNIT `<in_limit>` budget, and a time-unit-block initial fill.
    discrete: bool,
    capacity: Option<&'a str>,
    in_limit: Option<&'a str>,
    /// `<sample>` (§6.1): re-latch `<len>` only on the steps this reads nonzero.
    sample: Option<&'a str>,
    /// `<arrest>` (§4.3 step 0): freeze the belt on the steps this reads nonzero.
    arrest: Option<&'a str>,
}

impl<'a> FlagBelt<'a> {
    /// The continuous, unconstrained, always-sampling belt every flag test starts
    /// from -- so a test's `..` override names exactly the flag it is about.
    const fn plain(
        dt: &'a str,
        stop: &'a str,
        initial: &'a str,
        len: &'a str,
        inflow: &'a str,
    ) -> Self {
        FlagBelt {
            dt,
            stop,
            initial,
            len,
            inflow,
            discrete: false,
            capacity: None,
            in_limit: None,
            sample: None,
            arrest: None,
        }
    }
}

/// A one-belt model whose `<conveyor>` carries the step-3 flags. Reuses
/// [`leaky_belt_xmile`]'s shape (belt / `in_f` / `out_f` / `sink`) with no leaks.
fn flag_belt_xmile(spec: FlagBelt<'_>) -> String {
    let attrs = if spec.discrete {
        r#"discrete="true""#
    } else {
        ""
    };
    let tag = |name: &str, v: Option<&str>| {
        v.map(|v| format!("<{name}>{v}</{name}>"))
            .unwrap_or_default()
    };
    // The XMILE schema fixes the child order: capacity, in_limit, sample, arrest.
    let extra = format!(
        "{}{}{}{}",
        tag("capacity", spec.capacity),
        tag("in_limit", spec.in_limit),
        tag("sample", spec.sample),
        tag("arrest", spec.arrest),
    );
    leaky_belt_xmile(
        BeltXmile {
            dt: spec.dt,
            stop: spec.stop,
            initial: spec.initial,
            len: spec.len,
            inflow: spec.inflow,
            conv_attrs: attrs,
            conv_extra: &extra,
        },
        &[],
    )
}

/// `<sample>` (§6.1) gates the mid-run re-latch of `<len>`: on a step where the
/// sample expression reads zero the belt keeps the entry depth it last latched, so
/// a `<len>` that changed in between is never seen. There is no "next sample time"
/// to carry -- the expression is an ordinary aux the Flows phase re-evaluates -- and
/// the latched transit needs no storage either, because the only thing the VM
/// derives from it is the entry depth the descriptor already holds.
///
/// `<len>` doubles at t = 2 while `<sample>` has read 0 since t = 1, so the sampled
/// belt never grows. Its always-sampling twin does, which is what makes the test
/// non-vacuous: a lowering that ignored `<sample>` outright would still match the VM
/// on any belt whose `<len>` happened to be constant.
#[test]
fn sample_latches_the_transit_between_samples() {
    const LEN: &str = "2 + STEP(2, 2)";
    let series = |sample: Option<&str>| {
        let project = parse(&flag_belt_xmile(FlagBelt {
            sample,
            ..FlagBelt::plain("0.5", "6", "0", LEN, "TIME + 1")
        }));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
        wasm_series(&artifact, &run_artifact(&artifact), "out_f")
    };

    let latched = series(Some("1 - STEP(1, 1)"));
    let always = series(None);
    assert_ne!(
        latched, always,
        "<sample> must suppress the t = 2 growth the twin sees"
    );
    // An explicit, always-true `<sample>` is the absent tag: the `unwrap_or(true)`
    // default and a `sample_off` holding 1 must reach the same latch.
    assert_eq!(series(Some("1")), always);
    // ... and an always-false one freezes the entry depth at its INIT value, which
    // `init_belts` reads from the raw `<len>` regardless of `<sample>`.
    let never = series(Some("0"));
    assert_ne!(never, always);
}

/// The mid-run bound check lives INSIDE the `<sample>` gate (`run_phase_a`'s
/// `!arrested && sample && transit.is_finite()`): a step that does not re-latch never
/// evaluates the new slat count, so a `<len>` that grew past `slat_bound()` while the
/// belt was not sampling raises nothing.
///
/// This is also the shape a `select`-based narrowing would TRAP on rather than skip:
/// `select` evaluates both arms, so the un-narrowed `i32.trunc_f64_s` would run on the
/// slat count of an arbitrarily large `<len>`. The gate must be an `if`.
#[test]
fn sample_gates_the_midrun_bound_check() {
    let _guard = SlatBoundGuard::new(4);
    // <len> = 0.25 + t*0.25 would cross 4 slats at t = 3.5 (see
    // `midrun_transit_too_long_raises_like_vm`, the always-sampling twin of this
    // model, which raises). <sample> reads 0 from t = 0.5 on, so the last latch --
    // at t = 0.25, transit 0.3125 -- fixes the belt at 1 slat for the whole run.
    let project = parse(&flag_belt_xmile(FlagBelt {
        sample: Some("1 - STEP(1, 0.5)"),
        ..FlagBelt::plain("0.25", "6", "0", "0.25 + TIME * 0.25", "10")
    }));
    let main = project.models[0].name.clone();
    build_vm(&project, &main)
        .expect("build")
        .run_to_end()
        .expect("growth the belt never samples must not raise");

    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        assert_eq!(get_error(store, inst), 0, "no raise on a non-sampling step");
    });
}

/// The converse: a belt that DOES sample bound-checks every step it samples, and the
/// blob's raise reconstructs the VM's message byte for byte. Distinct from
/// [`midrun_transit_too_long_raises_like_vm`] in that `sample_off` is `Some` here, so
/// the raise sits under a runtime `if` rather than a compile-time-true one.
#[test]
fn an_explicitly_sampling_belt_still_raises_midrun() {
    let _guard = SlatBoundGuard::new(4);
    let project = parse(&flag_belt_xmile(FlagBelt {
        sample: Some("1"),
        ..FlagBelt::plain("0.25", "6", "0", "0.25 + TIME * 0.25", "10")
    }));
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    let vm_err = vm
        .run_to_end()
        .expect_err("the growing belt must trip the bound");
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
    });
}

/// `<arrest>` (§4.3 step 0) freezes the belt for the steps it reads nonzero: no
/// latch, no leak, no exit, and no admission. Every driven rate the belt owns is
/// published as 0, so the ordinary Stocks phase integrates a Δ of exactly 0 and the
/// flat stock holds still with the slats.
///
/// The inflow RAMPS so each slat holds a distinct volume: on a steady belt the frozen
/// ring would be indistinguishable from a shifted one.
#[test]
fn arrest_freezes_the_belt_and_publishes_zero_rates() {
    // Arrested for 1 <= t < 3, i.e. the steps at t = 1.0, 1.5, 2.0, 2.5.
    let project = parse(&flag_belt_xmile(FlagBelt {
        arrest: Some("STEP(1, 1) - STEP(1, 3)"),
        ..FlagBelt::plain("0.5", "5", "0", "1", "TIME + 4")
    }));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);

    let slab = run_artifact(&artifact);
    let (in_f, out_f, belt) = (
        wasm_series(&artifact, &slab, "in_f"),
        wasm_series(&artifact, &slab, "out_f"),
        wasm_series(&artifact, &slab, "belt"),
    );
    for c in 0..belt.len() {
        let t = 0.5 * c as f64;
        if (1.0..3.0).contains(&t) {
            assert_eq!(in_f[c], 0.0, "an arrested belt admits nothing at t={t}");
            assert_eq!(
                out_f[c], 0.0,
                "an arrested belt discharges nothing at t={t}"
            );
        }
    }
    // Row `c` records `curr` AFTER step `c`'s pass and stock integration, so the
    // stock is unchanged across every arrested step: rows 2 (t=1) through 6 (t=3).
    for c in 2..=6 {
        assert_eq!(belt[c], belt[2], "the stock moved while arrested (row {c})");
    }
    // Non-vacuously: it moved before, and it moves again after.
    assert!(belt[2] > 0.0 && out_f[8] > 0.0);
}

/// An arrested belt does not leak either -- `phase_a`'s step-0 return precedes
/// `leak_step`, so the `<leak_integers/>` carry is not advanced and the leak rate is
/// published as 0. What arrest does NOT freeze is the §6.3 time-unit budget reset
/// (the clock runs on) nor `step_contents0` (assigned before the return).
#[test]
fn arrest_stops_the_leak_and_leaves_the_carry_alone() {
    let project = parse(&leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "5",
            initial: "20",
            len: "1",
            inflow: "TIME + 4",
            conv_attrs: "",
            conv_extra: "<arrest>STEP(1, 1) - STEP(1, 3)</arrest>",
        },
        &[Leak::integer("0.4")],
    ));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

    let slab = run_artifact(&artifact);
    let leak = wasm_series(&artifact, &slab, "leak_0");
    let sunk = wasm_series(&artifact, &slab, "leak_sink_0");
    for (c, rate) in leak.iter().enumerate() {
        let t = 0.5 * c as f64;
        if (1.0..3.0).contains(&t) {
            assert_eq!(*rate, 0.0, "an arrested belt leaks nothing at t={t}");
        }
    }
    // The integer carry survives the arrest rather than being flushed or re-zeroed:
    // the leaked total resumes from where it stopped, and it did move before.
    assert!(sunk[2] > 0.0, "the belt leaked before the arrest");
    assert_eq!(sunk[2], sunk[6], "material leaked while arrested");
    assert!(sunk[9] > sunk[6], "the leak resumed after the arrest");
}

/// A LEAK flow whose destination belt is arrested is skipped, while the belt's OTHER
/// leaks and its exit run normally (`leak_step`'s per-flow `arrested(k)` guard). The
/// material stays on the upstream belt rather than vanishing.
#[test]
fn a_leak_into_an_arrested_belt_is_skipped() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt_z"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_z</outflow>
      <outflow>spill</outflow><conveyor><len>1</len></conveyor></stock>
    <stock name="belt_a"><eqn>0</eqn><inflow>spill</inflow><outflow>out_a</outflow>
      <conveyor><len>1</len><arrest>STEP(1, 1) - STEP(1, 3)</arrest></conveyor></stock>
    <flow name="in_f"><eqn>TIME + 4</eqn></flow>
    <flow name="spill"><eqn>0.4</eqn><leak/></flow>
    <flow name="out_z"></flow>
    <flow name="out_a"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_z</inflow><inflow>out_a</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 6);

    let slab = run_artifact(&artifact);
    let spill = wasm_series(&artifact, &slab, "spill");
    let out_z = wasm_series(&artifact, &slab, "out_z");
    for (c, rate) in spill.iter().enumerate() {
        let t = 0.5 * c as f64;
        if (1.0..3.0).contains(&t) {
            assert_eq!(*rate, 0.0, "leak into an arrested belt at t={t}");
            assert!(out_z[c] > 0.0, "the upstream belt's own exit still runs");
        }
    }
    assert!(spill[1] > 0.0 && spill[7] > 0.0, "the leak runs otherwise");

    // Nothing was destroyed: the skipped leak stays on belt_z.
    let last = spill.len() - 1;
    let at = |n: &str| wasm_series(&artifact, &slab, n)[last];
    let admitted: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((at("belt_z") + at("belt_a") + at("sink") - admitted).abs() < EPS);
}

/// A belt whose primary destination is an ARRESTED belt has its exit HELD (§4.3
/// step 3): `out_vol = 0`, but the latch and the leak still run, and phase B MERGES
/// the exit slat into its neighbour instead of popping it. Material piles up at the
/// front and discharges as one pulse when the destination releases.
///
/// This is the case that distinguishes held from arrested: an arrested belt would
/// have frozen its ring, so the pile-up would never form.
#[test]
fn a_held_exit_merges_at_the_front_and_releases() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt_a"><eqn>0</eqn><inflow>in_f</inflow><outflow>a_out</outflow>
      <conveyor><len>1</len></conveyor></stock>
    <stock name="belt_b"><eqn>0</eqn><inflow>a_out</inflow><outflow>b_out</outflow>
      <conveyor><len>1</len><arrest>STEP(1, 1) - STEP(1, 3)</arrest></conveyor></stock>
    <flow name="in_f"><eqn>TIME + 4</eqn></flow>
    <flow name="a_out"></flow>
    <flow name="b_out"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>b_out</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 6);

    let slab = run_artifact(&artifact);
    let a_out = wasm_series(&artifact, &slab, "a_out");
    let belt_a = wasm_series(&artifact, &slab, "belt_a");
    // While belt_b is arrested (t in [1, 3)) belt_a's exit is held...
    for (c, rate) in a_out.iter().enumerate() {
        let t = 0.5 * c as f64;
        if (1.0..3.0).contains(&t) {
            assert_eq!(*rate, 0.0, "the held exit discharges nothing at t={t}");
        }
    }
    // ... but belt_a keeps ADMITTING, so it grows through the hold -- an arrested
    // belt_a would have held its contents flat instead.
    assert!(belt_a[6] > belt_a[2] + EPS, "the held belt kept admitting");
    // The merged exit slat then leaves as one pulse, larger than any steady discharge.
    let steady = a_out[1];
    assert!(
        a_out[6] > 2.0 * steady,
        "the held-up cohort must discharge as a pulse: {a_out:?}"
    );

    let last = a_out.len() - 1;
    let at = |n: &str| wasm_series(&artifact, &slab, n)[last];
    let admitted: f64 = wasm_series(&artifact, &slab, "in_f")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((at("belt_a") + at("belt_b") + at("sink") - admitted).abs() < EPS);
}

// ── discrete belts (§6.4) ────────────────────────────────────────────────────

/// §6.4 rule 3: a discrete belt's initial fill is swept, block by block, onto each
/// time-unit block's DEEPEST slat. A 4-slat belt at dt = 0.5 spans two blocks, so the
/// steady fill of 25 per slat becomes `[0, 50, 0, 50]` (exit-first) and drains in
/// pulses rather than evenly.
///
/// The continuous twin drains at a flat 50, which is the whole point: matching the VM
/// on a discrete belt with `merge_time_unit_blocks` stubbed out would look identical
/// to a continuous one.
#[test]
fn discrete_belt_lumps_its_initial_fill_into_time_unit_blocks() {
    let out_of = |discrete: bool| {
        let project = parse(&flag_belt_xmile(FlagBelt {
            discrete,
            ..FlagBelt::plain("0.5", "4", "100", "2", "0")
        }));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
        wasm_series(&artifact, &run_artifact(&artifact), "out_f")
    };
    // The exit slat is empty at t = 0, so the first pulse lands at t = 0.5 (50 / dt).
    assert_eq!(out_of(true)[..6], [0.0, 100.0, 0.0, 100.0, 0.0, 0.0]);
    assert_eq!(out_of(false)[..6], [50.0, 50.0, 50.0, 50.0, 0.0, 0.0]);
}

/// §6.4 rule 1: a discrete belt admits only WHOLE units of an equation-driven inflow,
/// carrying the fraction to the next DT. An offer of 0.4 per DT therefore boards one
/// unit every third step, and the belt's published inflow rate pulses 0, 0, 1/dt.
///
/// The carry is per-inflow state that no ring holds, so it is one of the three things
/// the mid-run preview must save explicitly (see
/// [`midrun_preview_rolls_back_the_quant_carry_and_the_time_unit`]).
#[test]
fn discrete_admission_quantizes_and_carries() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        ..FlagBelt::plain("0.5", "5", "0", "1", "0.8")
    }));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
    let in_f = wasm_series(&artifact, &run_artifact(&artifact), "in_f");
    assert_eq!(in_f[..6], [0.0, 0.0, 2.0, 0.0, 2.0, 0.0]);

    // The continuous twin admits the offered rate every step; the discrete one is not
    // merely a rounded version of it.
    let cont = parse(&flag_belt_xmile(FlagBelt::plain(
        "0.5", "5", "0", "1", "0.8",
    )));
    let ca = artifact_for(&cont);
    assert_eq!(wasm_series(&ca, &run_artifact(&ca), "in_f")[..3], [0.8; 3]);
}

/// §6.3: a DISCRETE belt's `<in_limit>` is a per-TIME-UNIT budget drawn down by
/// `in_carry`, reset when the modeled clock crosses an integer time unit -- not a
/// per-DT rate cap (which is what a continuous belt's `in_limit * dt` is).
///
/// `dt = 0.3` does not divide 1, so the boundary falls strictly BETWEEN steps and
/// `conveyor_time_unit` must compute it on the IDEAL grid --
/// `floor(start + round((t - start)/dt) * dt)` -- rather than from the drifted
/// accumulated `t`: at k = 4 the accumulated clock reads 1.1999999999999997, whose
/// floor is 0, while the ideal 4*0.3 = 1.2000000000000002 floors to 1. A lowering
/// that floored the raw clock would delay every reset by one step.
///
/// With `<in_limit>` 1 against an offer of 3 per DT the belt admits one unit at the
/// first step of each time unit and nothing for the rest of it: k = 0, 4, 7, 10.
#[test]
fn discrete_in_limit_is_a_per_time_unit_budget() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        in_limit: Some("1"),
        ..FlagBelt::plain("0.3", "3", "0", "0.9", "10")
    }));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
    let in_f = wasm_series(&artifact, &run_artifact(&artifact), "in_f");
    assert!(
        in_f.len() >= 11,
        "expected at least 11 steps, got {}",
        in_f.len()
    );
    let admitted: Vec<bool> = in_f[..11].iter().map(|r| *r > 0.0).collect();
    assert_eq!(
        admitted,
        [
            true, false, false, false, // time unit 0: t = 0, 0.3, 0.6, 0.9
            true, false, false, // time unit 1: t = 1.2, 1.5, 1.8
            true, false, false, // time unit 2: t = 2.1, 2.4, 2.7
            true,  // time unit 3: t = 3.0
        ],
        "{in_f:?}"
    );

    // A CONTINUOUS belt with the same `<in_limit>` admits `1 * dt` every single DT --
    // the two readings of the same tag, and the reason this cannot be shared code.
    let cont = parse(&flag_belt_xmile(FlagBelt {
        in_limit: Some("1"),
        ..FlagBelt::plain("0.3", "3", "0", "0.9", "10")
    }));
    let ca = artifact_for(&cont);
    assert_eq!(wasm_series(&ca, &run_artifact(&ca), "in_f")[..4], [1.0; 4]);
}

/// A discrete belt's `<capacity>` also quantizes: `discrete_admit` floors the capacity
/// room into a whole-unit budget shared across the inflows in listed order. With room
/// for 2.5 units only 2 board, and the belt never exceeds a whole-unit fill.
#[test]
fn discrete_admission_floors_the_capacity_budget() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        capacity: Some("2.5"),
        ..FlagBelt::plain("0.5", "4", "0", "1", "10")
    }));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
    let slab = run_artifact(&artifact);
    let belt = wasm_series(&artifact, &slab, "belt");
    for (c, v) in belt.iter().enumerate() {
        assert_eq!(*v, v.floor(), "row {c}: a discrete belt holds whole units");
        assert!(*v <= 2.5 + EPS, "row {c}: {v} exceeds <capacity>");
    }
    assert!(belt.contains(&2.0), "the belt did fill: {belt:?}");
}

/// §7.2 + §6.4 rule 3: an explicit per-slat init list on a discrete belt is filled
/// slat-by-slat and THEN swept onto each block's deepest slat, so `10,20,30,40` on a
/// 4-slat / 2-block belt becomes `[0, 30, 0, 70]` and leaves in two pulses.
#[test]
fn discrete_explicit_init_list_per_slat_matches_vm() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        ..FlagBelt::plain("0.5", "3", "10,20,30,40", "2", "0")
    }));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
    let out_f = wasm_series(&artifact, &run_artifact(&artifact), "out_f");
    assert_eq!(out_f[..5], [0.0, 60.0, 0.0, 140.0, 0.0]);
}

/// §7.2: a per-TIME-UNIT init list (one entry per block, not per slat) on a DISCRETE
/// belt places each entry whole on its block's deepest slat, where the continuous belt
/// spreads it evenly across the block's slats. Both fills are lowered, and they differ.
#[test]
fn discrete_explicit_init_list_per_time_unit_matches_vm() {
    let out_of = |discrete: bool| {
        let project = parse(&flag_belt_xmile(FlagBelt {
            discrete,
            ..FlagBelt::plain("0.5", "3", "10,20", "2", "0")
        }));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 4);
        wasm_series(&artifact, &run_artifact(&artifact), "out_f")
    };
    assert_eq!(out_of(true)[..5], [0.0, 20.0, 0.0, 40.0, 0.0]);
    assert_eq!(out_of(false)[..5], [10.0, 10.0, 20.0, 20.0, 0.0]);
}

/// A discrete and a continuous belt in ONE model: the pass is specialized per plan,
/// so the shared `b_merge_blocks` / `b_round` helpers are emitted once and applied only
/// where the plan says. A per-model (rather than per-plan) `discrete` flag would make
/// the two belts agree, which they must not.
#[test]
fn mixed_continuous_and_discrete_belts_match_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>5</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="belt_c"><eqn>60</eqn><inflow>in_c</inflow><outflow>out_c</outflow>
      <conveyor><len>1.5</len><in_limit>2</in_limit></conveyor></stock>
    <stock name="belt_d"><eqn>60</eqn><inflow>in_d</inflow><outflow>out_d</outflow>
      <conveyor discrete="true"><len>1.5</len><in_limit>2</in_limit></conveyor></stock>
    <flow name="in_c"><eqn>TIME + 1</eqn></flow>
    <flow name="in_d"><eqn>TIME + 1</eqn></flow>
    <flow name="out_c"></flow>
    <flow name="out_d"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_c</inflow><inflow>out_d</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 7);
    let slab = run_artifact(&artifact);
    assert_ne!(
        wasm_series(&artifact, &slab, "belt_c"),
        wasm_series(&artifact, &slab, "belt_d"),
        "the discrete belt must not behave like its continuous twin"
    );
}

/// The mid-run preview must roll back BOTH of the discrete belt's static-region
/// carries: `quant_carry` (§6.4 rule 1, per inflow) and `Vm::conveyor_last_unit`
/// (§6.3, one per model). Neither rides the ring clone or the descriptor save, so each
/// needs an explicit save/restore -- exactly like `leak_carry`.
///
/// Every `run_to` boundary below rests on a different phase of the time unit, so at
/// least one preview crosses an integer boundary and consumes it. If `last_unit` were
/// not restored, the resumed step would find `unit == last_unit`, skip the §6.3 budget
/// reset, and admit less; if `quant_carry` were not restored, the resumed step's
/// clearance would start from a pre-advanced fraction. Either way the segmented slab
/// diverges from the single-`run` one.
///
/// `<in_limit>` and a fractional offer are both required: an unconstrained belt has no
/// `in_carry` to reset, and a whole-unit offer leaves no fraction to carry.
#[test]
fn midrun_preview_rolls_back_the_quant_carry_and_the_time_unit() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        in_limit: Some("3"),
        ..FlagBelt::plain("0.5", "4", "0", "1", "1.4")
    }));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 4);

    let segmented = with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        for target in [0.5f64, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0] {
            call_run_to(store, inst, target);
        }
        assert_eq!(get_error(store, inst), 0);
        read_slab(store, inst, &artifact)
    });
    assert_eq!(
        single, segmented,
        "the preview leaked quant_carry or conveyor_last_unit into the real belt"
    );

    // Non-vacuity: the belt really does carry a fraction and really does reset at a
    // boundary, so a preview that advanced either would have been visible.
    let in_f = wasm_series(&artifact, &single, "in_f");
    assert!(
        in_f.contains(&0.0) && in_f.iter().any(|r| *r > 0.0),
        "the admission must pulse for this test to bite: {in_f:?}"
    );
}

/// `reset` re-zeroes the discrete carries along with the leak carry, so repeated `run`s
/// over a discrete belt reproduce the identical slab. Without it the second run would
/// start mid-fraction and mid-time-unit.
#[test]
fn reset_rezeroes_the_discrete_carries() {
    let project = parse(&flag_belt_xmile(FlagBelt {
        discrete: true,
        in_limit: Some("3"),
        ..FlagBelt::plain("0.5", "4", "0", "1", "1.4")
    }));
    let artifact = artifact_for(&project);
    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run");
        let first = read_slab(store, inst, &artifact);
        call_void(store, inst, "run");
        let second = read_slab(store, inst, &artifact);
        assert_eq!(first, second, "the discrete carries survived `reset`");
    });
}

// ── queue -> discrete conveyor coupling (§11 / queues.md §9) ─────────────────

/// A queue whose primary outflow is a discrete belt's equation-driven inflow. The belt
/// attributes (`one_at_a_time`, `batch_integrity`) select the batch rule the coupled
/// serve takes under; `conv_extra` splices `<conveyor>` children after `<capacity>`
/// (`<in_limit>`, `<sample>`, `<arrest>` -- in that schema order); `q_extra` turns the
/// queue's second outflow on (pass `<overflow/>` or `""`).
fn coupled_xmile(conv_attrs: &str, capacity: &str, conv_extra: &str, q_extra: &str) -> String {
    let (q_outflow, q_vars) = if q_extra.is_empty() {
        (String::new(), String::new())
    } else {
        (
            "<outflow>spill</outflow>".to_string(),
            format!(
                r#"<flow name="spill"><eqn>0</eqn>{q_extra}</flow>
                   <stock name="spilled"><eqn>0</eqn><inflow>spill</inflow></stock>"#
            ),
        )
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/><uses_queue overflow="true"/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="q"><eqn>0</eqn><inflow>arrivals</inflow><outflow>q_out</outflow>
      {q_outflow}<queue/></stock>
    <flow name="arrivals"><eqn>TIME + 1</eqn></flow>
    <flow name="q_out"><eqn>0</eqn></flow>
    {q_vars}
    <stock name="belt"><eqn>0</eqn><inflow>q_out</inflow><outflow>out_f</outflow>
      <conveyor discrete="true" {conv_attrs}><len>1</len><capacity>{capacity}</capacity>
        {conv_extra}</conveyor></stock>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
    )
}

/// All four `(one_at_a_time, batch_integrity)` batch rules of a coupled serve, each
/// diffed against the VM -- and each producing a DIFFERENT answer, which is what proves
/// the four arms are specialized rather than all collapsing onto `take_from_front`.
///
/// The `<capacity>` schedule is what makes the rules bite. It is 2 until t = 2, so the
/// queue builds a backlog of several batches; then it jumps to 22, and the two
/// `one_at_a_time = false` rules can reach PAST the front batch while the two
/// `one_at_a_time = true` rules still cannot. On an unconstrained belt -- or on one that
/// never lets a second batch accumulate -- all four rules take the front batch whole and
/// agree vacuously, which is exactly what a first draft of this test did.
#[test]
fn every_coupled_batch_rule_matches_vm() {
    let series = |attrs: &str| {
        let project = parse(&coupled_xmile(attrs, "2 + STEP(20, 2)", "", ""));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

        let slab = run_artifact(&artifact);
        // Conservation: every unit that arrived is in the queue, on the belt, or sunk.
        let last = artifact.layout.n_chunks - 1;
        let at = |n: &str| wasm_series(&artifact, &slab, n)[last];
        let arrived: f64 = wasm_series(&artifact, &slab, "arrivals")
            .iter()
            .take(last)
            .map(|r| r * 0.5)
            .sum();
        assert!(
            (at("q") + at("belt") + at("sink") - arrived).abs() < EPS,
            "{attrs}: material was destroyed"
        );
        wasm_series(&artifact, &slab, "belt")
    };

    // `one_at_a_time` defaults to true and `batch_integrity` to false.
    let split_front = series("");
    let whole_front = series(r#"batch_integrity="true""#);
    let split_many = series(r#"one_at_a_time="false""#);
    let whole_many = series(r#"one_at_a_time="false" batch_integrity="true""#);

    // All four are pairwise distinct. No ORDERING is asserted between them: the belt
    // stock is history-dependent, so a rule that boarded less this DT can hold more the
    // next. "Batch integrity takes at most what a splitting take would" is a statement
    // about one serve at one queue state, not about the resulting series.
    let all = [&split_front, &whole_front, &split_many, &whole_many];
    for a in 0..all.len() {
        for b in a + 1..all.len() {
            assert_ne!(all[a], all[b], "batch rules {a} and {b} must differ");
        }
    }
}

/// BOTH `batch_integrity` rules are INCLUSIVE at the budget: a batch of exactly `req`
/// boards whole. `front <= req` in the single-front-batch arm, `front <= budget` in the
/// whole-batches loop -- never a strict `<`, under which a batch that exactly fills the
/// belt's remaining room would board nothing, forever.
///
/// Nothing else in the suite reaches that boundary. `every_coupled_batch_rule_matches_vm`
/// only ever offers a front batch STRICTLY smaller than `req`, so it cannot tell `<=`
/// from `<`; a batch exactly filling the room is a measure-zero coincidence unless the
/// model is built for it. Both arms therefore need their own case here -- they are
/// separate emitted comparisons, and a mutation that weakens one leaves the other intact.
///
/// Constant arrivals of 4 at `dt = 0.5` make each DT's batch exactly 2, which is exactly
/// the empty belt's `<capacity>` of 2. `one_at_a_time` defaults to true, so the first
/// case exercises the single-front-batch arm and the second the whole-batches loop.
#[test]
fn both_batch_integrity_takes_are_inclusive_at_the_exact_budget() {
    for attrs in [
        r#"batch_integrity="true""#,
        r#"one_at_a_time="false" batch_integrity="true""#,
    ] {
        let project = parse(&coupled_xmile(attrs, "2", "", "").replace("TIME + 1", "4"));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

        let slab = run_artifact(&artifact);
        let q_out = wasm_series(&artifact, &slab, "q_out");
        assert_eq!(
            q_out[0], 4.0,
            "{attrs}: a batch of exactly `req` must board: {q_out:?}"
        );
        // And it keeps boarding as the belt's exit frees the same room again.
        assert!(
            q_out.iter().filter(|r| **r > 0.0).count() >= 3,
            "{attrs}: {q_out:?}"
        );
    }
}

/// The coupled serve is interleaved between the belt's phase A and its phase B, so the
/// room this DT's exit and leak freed is available to the queue THIS DT (`queues.md`
/// §9). A serve emitted after phase B -- or before phase A -- would see a full belt and
/// admit nothing on the steady state.
///
/// It is also a pass-through: material arriving at the queue this DT can board the belt
/// the same DT, because `admit_inflows` runs before the take.
#[test]
fn a_coupled_serve_sees_the_room_this_steps_exit_freed() {
    // A belt of capacity 2 that discharges 1 per DT in the steady state: with the
    // serve interleaved it keeps taking 1 per DT forever. `arrivals` is a constant
    // so the steady state is unambiguous.
    let project = parse(&coupled_xmile("", "2", "", "").replace("TIME + 1", "1"));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

    let slab = run_artifact(&artifact);
    let q_out = wasm_series(&artifact, &slab, "q_out");
    let q = wasm_series(&artifact, &slab, "q");
    // The queue never backs up: everything that arrives boards within the same DT.
    assert!(
        q.iter().all(|v| *v < EPS),
        "the belt stopped taking: the serve saw a stale belt {q:?}"
    );
    assert!(
        q_out.iter().skip(1).all(|r| (*r - 1.0).abs() < EPS),
        "{q_out:?}"
    );
}

/// An ARRESTED belt requests nothing (`admission_budget`'s first line), so its coupled
/// queue holds its batches rather than draining them to a cloud. The queue backs up
/// while the belt is arrested and drains again when it releases.
#[test]
fn an_arrested_belt_holds_its_coupled_queue() {
    // `one_at_a_time="false"` so the released belt drains the whole backlog and the
    // "did the queue actually back up?" check below is unambiguous; the default rule
    // would move one batch per DT and never empty a queue that keeps receiving.
    let project = parse(&coupled_xmile(
        r#"one_at_a_time="false""#,
        "1e9",
        "<arrest>STEP(1, 1) - STEP(1, 3)</arrest>",
        "",
    ));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

    let slab = run_artifact(&artifact);
    let q_out = wasm_series(&artifact, &slab, "q_out");
    let q = wasm_series(&artifact, &slab, "q");
    for (c, rate) in q_out.iter().enumerate() {
        let t = 0.5 * c as f64;
        if (1.0..3.0).contains(&t) {
            assert_eq!(*rate, 0.0, "an arrested belt takes nothing at t={t}");
        }
    }
    // The queue really did back up (and so was not draining anyway), then drained.
    assert!(
        q[5] > q[2] + EPS,
        "the queue must accumulate under arrest: {q:?}"
    );
    assert!(q[7] < EPS, "the queue must drain on release: {q:?}");
}

/// A coupled queue's `<overflow/>` sibling claims exactly `desire − taken`: the front
/// material the belt's budget refused this DT (`queues.md` §4.5). The desire must be
/// measured on the PRE-take front -- measuring it after would always yield 0 and the
/// overflow would never fire.
#[test]
fn a_coupled_queues_overflow_claims_the_refused_volume() {
    let project = parse(&coupled_xmile("", "1", "", "<overflow/>"));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 6);

    let slab = run_artifact(&artifact);
    let spill = wasm_series(&artifact, &slab, "spill");
    assert!(
        spill.iter().any(|r| *r > EPS),
        "the capacity-blocked volume must overflow: {spill:?}"
    );
    // Conservation across all three sinks.
    let last = artifact.layout.n_chunks - 1;
    let at = |n: &str| wasm_series(&artifact, &slab, n)[last];
    let arrived: f64 = wasm_series(&artifact, &slab, "arrivals")
        .iter()
        .take(last)
        .map(|r| r * 0.5)
        .sum();
    assert!((at("q") + at("belt") + at("sink") + at("spilled") - arrived).abs() < EPS);
}

/// A coupled take is charged against the belt's DISCRETE per-time-unit `<in_limit>`
/// budget (§6.3/§11) by `consume_inflow_budget`, not by `phase_b`'s equation-inflow
/// accounting -- the coupled volume never passes through `discrete_admit`. Without that
/// debit every DT inside a time unit would see the full budget and the belt would admit
/// `in_limit` per DT rather than per time unit.
///
/// `dt = 0.5`, so a missing debit would admit twice as much per time unit. The budget of
/// 1 against an unbounded queue backlog pins the rate exactly.
#[test]
fn a_coupled_take_draws_down_the_per_time_unit_inflow_budget() {
    let project = parse(&coupled_xmile(
        r#"one_at_a_time="false""#,
        "1e9",
        "<in_limit>1</in_limit>",
        "",
    ));
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 5);

    let slab = run_artifact(&artifact);
    let q_out = wasm_series(&artifact, &slab, "q_out");
    // Arrivals ramp, so the queue always has more than the budget on offer after the
    // first step: exactly one unit boards per time unit, at that unit's first DT.
    for (c, rate) in q_out.iter().enumerate().take(10).skip(2) {
        let want = if c % 2 == 0 { 2.0 } else { 0.0 };
        assert_eq!(*rate, want, "row {c} of {q_out:?}");
    }
    // The time-unit reset is what lets it board again; a belt whose carry never reset
    // would board once and then never.
    assert!(q_out.iter().filter(|r| **r > 0.0).count() >= 4, "{q_out:?}");
}

/// TWO queues feeding one belt are served in the BELT's `<inflow>` declaration order,
/// each sizing its budget against the room its predecessors already took
/// (`prior_coupled_vol`). Swapping the two `<inflow>` tags therefore swaps which queue
/// gets the scarce capacity -- and nothing else about the model changes.
///
/// This is the only test that can distinguish `prior_coupled_vol` from 0: with one
/// queue, or with slack capacity, the accumulator is unobservable.
#[test]
fn two_queues_feeding_one_belt_use_the_inflow_declaration_order() {
    let model = |inflows: &str| {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/><uses_queue/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>5</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="q1"><eqn>0</eqn><inflow>arr1</inflow><outflow>q1_out</outflow><queue/></stock>
    <stock name="q2"><eqn>0</eqn><inflow>arr2</inflow><outflow>q2_out</outflow><queue/></stock>
    <flow name="arr1"><eqn>3</eqn></flow>
    <flow name="arr2"><eqn>5</eqn></flow>
    <flow name="q1_out"><eqn>0</eqn></flow>
    <flow name="q2_out"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn>{inflows}<outflow>out_f</outflow>
      <conveyor discrete="true" one_at_a_time="false"><len>1</len><capacity>2</capacity></conveyor></stock>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
  </variables></model>
</xmile>"#
        )
    };
    let series = |inflows: &str| {
        let project = parse(&model(inflows));
        let artifact = artifact_for(&project);
        assert!(assert_slab_matches_vm(&project, &artifact) >= 6);
        let slab = run_artifact(&artifact);
        (
            wasm_series(&artifact, &slab, "q1"),
            wasm_series(&artifact, &slab, "q2"),
        )
    };
    let (q1_first, q2_first) = series("<inflow>q1_out</inflow><inflow>q2_out</inflow>");
    let (q1_second, q2_second) = series("<inflow>q2_out</inflow><inflow>q1_out</inflow>");
    assert_ne!(
        q1_first, q1_second,
        "admission priority is not the <inflow> order"
    );
    assert_ne!(q2_first, q2_second);
    // The privileged queue backs up less than it does when it is served second.
    assert!(q1_first.last().unwrap() < q1_second.last().unwrap());
    assert!(q2_second.last().unwrap() < q2_first.last().unwrap());
}

/// A coupled model's mid-run preview is side-effect-free too: the interleaved pass runs
/// on cloned belt rings AND cloned batch rings, and the resumed run lands on the
/// byte-identical single-`run` slab. `prior_coupled_vol` / `req` / `taken` are `run_to`
/// locals, so nothing about them survives the preview to be restored.
#[test]
fn a_coupled_models_preview_is_side_effect_free() {
    let project = parse(&coupled_xmile("", "3", "", "<overflow/>"));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("vm");
    vm.run_to(3.0).expect("VM must run to the preview point");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 3.0);
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
        assert!(checked >= 6);

        call_run_to(store, inst, 6.0);
        assert_eq!(get_error(store, inst), 0);
        assert_eq!(single, read_slab(store, inst, &artifact));
    });
}

/// An UNCOUPLED queue in a model that also has a coupling is served by the plain
/// admit-then-serve tail, exactly once. Serving it inside the interleaved loop -- or
/// twice -- would double-admit its inflow.
#[test]
fn an_uncoupled_queue_beside_a_coupled_one_is_served_once() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/><uses_queue/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="q"><eqn>0</eqn><inflow>arrivals</inflow><outflow>q_out</outflow><queue/></stock>
    <stock name="free_q"><eqn>2</eqn><inflow>free_in</inflow><outflow>free_out</outflow><queue/></stock>
    <flow name="arrivals"><eqn>TIME + 1</eqn></flow>
    <flow name="free_in"><eqn>3</eqn></flow>
    <flow name="q_out"><eqn>0</eqn></flow>
    <flow name="free_out"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>q_out</inflow><outflow>out_f</outflow>
      <conveyor discrete="true"><len>1</len><capacity>2</capacity></conveyor></stock>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
    <stock name="free_sink"><eqn>0</eqn><inflow>free_out</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert!(assert_slab_matches_vm(&project, &artifact) >= 8);

    let slab = run_artifact(&artifact);
    // The uncoupled queue is a pass-through: it drains everything it admits, so its
    // stock returns to 0 and its outflow reports the admitted rate. Admitting twice
    // would double `free_out`.
    let free_out = wasm_series(&artifact, &slab, "free_out");
    assert_eq!(free_out[0], (2.0 + 3.0 * 0.5) / 0.5);
    assert!(
        free_out.iter().skip(1).all(|r| (*r - 3.0).abs() < EPS),
        "{free_out:?}"
    );
    // Row 0 records the stock AT t = 0 -- its initial value, before the step's Stocks
    // phase -- so the drained-to-empty invariant starts at row 1.
    let free_q = wasm_series(&artifact, &slab, "free_q");
    assert_eq!(free_q[0], 2.0);
    assert!(free_q.iter().skip(1).all(|v| *v < EPS), "{free_q:?}");
}

// ── container access (§10, the step-start hook point) ────────────────────────

/// A one-belt model whose slats are read through container-access variables (§10).
/// `containers` splices the reading auxes in; everything else is [`BeltXmile`].
///
/// The belt has no `<capacity>`/`<in_limit>`, so `in_f`'s reported value is the
/// requested rate -- which lets a test that feeds a container value back into `in_f`
/// read the admitted rate as an assertion on what the Flows phase saw.
fn container_belt_xmile(spec: BeltXmile<'_>, containers: &str) -> String {
    let BeltXmile {
        dt,
        stop,
        initial,
        len,
        inflow,
        conv_attrs,
        conv_extra,
    } = spec;
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>{stop}</stop><dt>{dt}</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>{initial}</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor {conv_attrs}><len>{len}</len>{conv_extra}</conveyor></stock>
    <flow name="in_f"><eqn>{inflow}</eqn></flow>
    <flow name="out_f"></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_f</inflow></stock>
    {containers}
  </variables></model>
</xmile>"#
    )
}

/// The `<aux>` block every kind-coverage test below reuses: all six reducers, the two
/// boundary slats, and two indices that can never name a slat.
const ALL_CONTAINERS: &str = r#"
    <aux name="n"><eqn>SIZE(belt)</eqn></aux>
    <aux name="total"><eqn>SUM(belt)</eqn></aux>
    <aux name="avg"><eqn>MEAN(belt)</eqn></aux>
    <aux name="lo"><eqn>MIN(belt)</eqn></aux>
    <aux name="hi"><eqn>MAX(belt)</eqn></aux>
    <aux name="spread"><eqn>STDDEV(belt)</eqn></aux>
    <aux name="exit_slat"><eqn>belt[1]</eqn></aux>
    <aux name="mid_slat"><eqn>belt[2]</eqn></aux>
    <aux name="entry_slat"><eqn>belt[4]</eqn></aux>
    <aux name="beyond"><eqn>belt[5]</eqn></aux>
    <aux name="zeroth"><eqn>belt[0]</eqn></aux>"#;

/// Every container-access kind over a four-slat belt whose slats hold DISTINCT
/// volumes, so `MEAN`/`MIN`/`MAX`/`STDDEV` are four different numbers and a reducer
/// that silently returned the belt total (or the exit slat) would be caught. A uniform
/// belt would make this test vacuous.
///
/// The §7.2 per-slat init list `1, 2, 3, 4` puts slat `j` (0 = exit) at volume `j + 1`;
/// with no inflow the belt then discharges one slat per DT, so the run also walks the
/// belt down to all-zero contents -- the reachable analogue of an "empty" container
/// (`SIZE` stays 4: `phase_b` regrows the ring to the entry depth every step).
#[test]
fn container_access_matches_vm() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.25",
            stop: "1.25",
            initial: "1, 2, 3, 4",
            len: "1",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        ALL_CONTAINERS,
    ));
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 11, "expected every container, checked {checked}");

    // Independent oracle, so this cannot pass vacuously if BOTH backends were wrong.
    // The exit-first slat vectors are [1,2,3,4], [2,3,4,0], [3,4,0,0], [4,0,0,0], then
    // all zeros: each step pops the exit slat and regrows an empty one at the entry.
    let slab = run_artifact(&artifact);
    let series = |name: &str| wasm_series(&artifact, &slab, name);

    assert_eq!(series("n"), vec![4.0; 6], "SIZE is the physical slat count");
    assert_eq!(series("total"), vec![10.0, 9.0, 7.0, 4.0, 0.0, 0.0]);
    assert_eq!(series("avg"), vec![2.5, 2.25, 1.75, 1.0, 0.0, 0.0]);
    assert_eq!(series("lo"), vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(series("hi"), vec![4.0, 4.0, 4.0, 4.0, 0.0, 0.0]);
    assert_eq!(series("exit_slat"), vec![1.0, 2.0, 3.0, 4.0, 0.0, 0.0]);
    assert_eq!(series("mid_slat"), vec![2.0, 3.0, 4.0, 0.0, 0.0, 0.0]);
    assert_eq!(series("entry_slat"), vec![4.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

    // POPULATION stddev (divisor N = 4), not the sample one (N - 1 = 3): the t=0
    // deviations from the mean 2.5 are +-1.5 and +-0.5, so the variance is 5/4.
    let spread = series("spread");
    assert!(
        (spread[0] - 1.25_f64.sqrt()).abs() < EPS,
        "STDDEV {spread:?}"
    );
    assert!(
        (spread[0] - (5.0_f64 / 3.0).sqrt()).abs() > 0.1,
        "a sample stddev (divisor N-1) would be {}",
        (5.0_f64 / 3.0).sqrt()
    );
    assert_eq!(spread[4], 0.0, "an all-zero belt has no spread");

    // A 1-based index past the live length, and the 1-based index 0, are both NaN --
    // the rule an out-of-range dynamic array subscript follows.
    assert!(
        series("beyond").iter().all(|v| v.is_nan()),
        "belt[5] over a 4-slat belt must be NaN, got {:?}",
        series("beyond")
    );
    assert!(
        series("zeroth").iter().all(|v| v.is_nan()),
        "belt[0] is out of range for a 1-based index, got {:?}",
        series("zeroth")
    );
}

/// A `conv[j]` whose 0-based index does not fit in an `i32` names no slat of any belt
/// (`reject_unsupported` pins `slat_bound() <= i32::MAX`), so it is NaN like any other
/// out-of-range index. The emitter must not narrow it with `as i32`: `2^32 + 1` would
/// WRAP to 0 and silently return the exit slat.
#[test]
fn a_slat_index_beyond_i32_is_nan() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "1",
            initial: "9",
            len: "1.5",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="exit_slat"><eqn>belt[1]</eqn></aux>
           <aux name="wrapped"><eqn>belt[4294967297]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // `(4294967297 - 1) as i32 == 0`, so a wrapping narrowing would report the exit
    // slat's 3.0 here rather than NaN.
    let exit = wasm_series(&artifact, &slab, "exit_slat");
    assert!((exit[0] - 3.0).abs() < EPS, "exit_slat = {exit:?}");
    let wrapped = wasm_series(&artifact, &slab, "wrapped");
    assert!(
        wrapped.iter().all(|v| v.is_nan()),
        "an index past i32::MAX must be NaN, got {wrapped:?}"
    );
}

/// The step-start publish and the between-Flows-and-Stocks pass are DISTINCT hook
/// points. Two independent consequences, each of which a mis-placed publish breaks:
///
/// 1. A Flows-phase reader of `SUM(belt)` sees the slats as the PREVIOUS step's pass
///    left them. `in_f = 2 + SUM(belt)` therefore reports exactly `2 + total` in the
///    SAME saved row. Publishing after the Flows call would feed it the row before.
/// 2. The saved row's `total` equals the saved row's belt contents. Publishing after
///    the pass would save the NEXT step's start state in this step's row.
///
/// The belt is a genuine feedback loop, so every row's contents differ -- an inflow
/// that ignored the container would leave `total` constant and pass vacuously.
#[test]
fn container_publish_is_start_of_step_not_post_pass() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "0",
            len: "1",
            inflow: "2 + SUM(belt)",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="total"><eqn>SUM(belt)</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let total = wasm_series(&artifact, &slab, "total");
    let belt = wasm_series(&artifact, &slab, "belt");
    let in_f = wasm_series(&artifact, &slab, "in_f");

    for (i, (&t, &b)) in total.iter().zip(belt.iter()).enumerate() {
        assert!(
            (t - b).abs() < EPS,
            "row {i}: SUM(belt) = {t} but the belt holds {b}; \
             the publish landed after the pass"
        );
        assert!(
            (in_f[i] - (2.0 + t)).abs() < EPS,
            "row {i}: in_f = {} but the Flows phase should have read SUM = {t}",
            in_f[i]
        );
    }
    // The loop must actually move, or the two assertions above hold for free.
    assert!(
        belt.windows(2).any(|w| (w[0] - w[1]).abs() > EPS),
        "a constant belt makes this test vacuous: {belt:?}"
    );
    assert!(belt[0] == 0.0 && belt[1] > 0.0, "belt = {belt:?}");
}

/// `INIT(<container access>)` must read the START-OF-RUN slat state, not the container
/// stock's frozen `0` placeholder. The `run_initials` reconciliation -- a second,
/// container-skipping initials pass plus a re-snapshot -- is what supplies that, and
/// the container slots joining `reconcile_skip_offsets` is what keeps the re-run from
/// clobbering them.
#[test]
fn container_init_reconciles_like_vm() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "2",
            initial: "60",
            len: "1.5",
            inflow: "0",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="init_sum"><eqn>INIT(SUM(belt))</eqn></aux>
           <aux name="init_size"><eqn>INIT(SIZE(belt))</eqn></aux>
           <aux name="init_slat"><eqn>INIT(belt[1])</eqn></aux>
           <aux name="ratio"><eqn>SUM(belt) / INIT(SUM(belt))</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Three slats (1.5 / 0.5) each holding 20, so the start-of-run reductions are
    // 60 / 3 / 20. Without the reconciliation every one of them would be 0.
    let slab = run_artifact(&artifact);
    for (name, want) in [("init_sum", 60.0), ("init_size", 3.0), ("init_slat", 20.0)] {
        for (i, &v) in wasm_series(&artifact, &slab, name).iter().enumerate() {
            assert!(
                (v - want).abs() < EPS,
                "step {i}: {name} = {v}, want {want}"
            );
        }
    }
    // Without the reconciliation this would be inf (a division by the frozen 0).
    let ratio = wasm_series(&artifact, &slab, "ratio");
    assert!(
        ratio.iter().all(|v| v.is_finite()),
        "INIT(SUM(belt)) must not be the frozen 0 placeholder, got {ratio:?}"
    );
}

/// A single-slat belt (`<len>` == `dt`): every reducer collapses onto the one slat, and
/// `belt[2]` is already past the end. The degenerate case the ring's `rem_u` addressing
/// and the reducers' `len`-bounded loops both have to survive.
#[test]
fn single_slat_belt_containers_match_vm() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "2",
            initial: "7",
            len: "0.5",
            inflow: "6",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="n"><eqn>SIZE(belt)</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="avg"><eqn>MEAN(belt)</eqn></aux>
           <aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>
           <aux name="spread"><eqn>STDDEV(belt)</eqn></aux>
           <aux name="only"><eqn>belt[1]</eqn></aux>
           <aux name="beyond"><eqn>belt[2]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let series = |name: &str| wasm_series(&artifact, &slab, name);
    assert_eq!(series("n"), vec![1.0; 5]);
    // Seeded with 7, then a rate of 6 delivers 3 per DT; the single slat discharges
    // wholly each step, so it holds 7 then 3 forever.
    assert_eq!(series("total"), vec![7.0, 3.0, 3.0, 3.0, 3.0]);
    for name in ["avg", "lo", "hi", "only"] {
        assert_eq!(series(name), series("total"), "{name} of a one-slat belt");
    }
    assert_eq!(series("spread"), vec![0.0; 5], "one slat has no spread");
    assert!(series("beyond").iter().all(|v| v.is_nan()));
}

/// Containers on a belt whose `<len>` grows and then shrinks: `SIZE` tracks the
/// PHYSICAL slat count -- which lags the entry depth on the way down, because
/// `ConveyorState::shift` only drops trailing slats that are EMPTY. `SUM` still equals
/// the belt's contents at every row.
#[test]
fn container_on_a_growing_and_shrinking_belt_matches_vm() {
    // 1 slat at t=0, up to 4 by t=1.5, back to 1 by t=3.
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "3",
            initial: "0",
            len: "0.5 + 0.5 * (1.5 - ABS(TIME - 1.5))",
            inflow: "4",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="n"><eqn>SIZE(belt)</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let n = wasm_series(&artifact, &slab, "n");
    let total = wasm_series(&artifact, &slab, "total");
    let belt = wasm_series(&artifact, &slab, "belt");
    assert!(
        n.windows(2).any(|w| w[1] > w[0]) && n.windows(2).any(|w| w[1] < w[0]),
        "the belt must both grow and shrink for this test to mean anything: {n:?}"
    );
    for (i, (&t, &b)) in total.iter().zip(belt.iter()).enumerate() {
        assert!((t - b).abs() < EPS, "row {i}: SUM = {t}, belt = {b}");
    }
    assert!(
        wasm_series(&artifact, &slab, "hi")
            .iter()
            .all(|v| *v >= -EPS),
        "MAX of a non-negative belt cannot go negative"
    );
}

/// The reducers divide and iterate over the PHYSICAL slat count `len`, never the entry
/// depth `d`. The two coincide on almost every belt, which is why this case needs its
/// own model: when `<len>` collapses onto a belt whose tail is FULL, `ConveyorState::
/// shift` refuses to drop the trailing slats (they hold material), so `len` stays above
/// `d` for as long as it takes the material to walk out.
///
/// `SIZE` reports `len`; `MEAN`'s divisor, the `MIN`/`MAX`/`STDDEV` folds, and
/// `conv[j]`'s bound check all use it too. Every one of those would still agree with the
/// VM on every other belt in this file if it read the descriptor's `d` instead, so each
/// is pinned here on the rows where the two differ.
#[test]
fn container_reduces_over_the_physical_length_not_the_entry_depth() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "5",
            initial: "0",
            // Six slats until t = 2, then two. The belt is full at the collapse.
            len: "IF TIME &lt; 2 THEN 3 ELSE 1",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="n"><eqn>SIZE(belt)</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="avg"><eqn>MEAN(belt)</eqn></aux>
           <aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="spread"><eqn>STDDEV(belt)</eqn></aux>
           <aux name="far_slat"><eqn>belt[5]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let series = |name: &str| wasm_series(&artifact, &slab, name);
    let (n, total, avg) = (series("n"), series("total"), series("avg"));
    let (lo, spread, far) = (series("lo"), series("spread"), series("far_slat"));

    // Rows 5 and 6 are t = 2.5 and t = 3.0, the first two publishes after the collapse.
    // The entry depth is 2 (`slat_count(1, 0.5)`) but cohorts of 5 are still walking
    // out behind it, so the slat vectors are [0,10,5,5,5] then [10,10,5,5]. A reducer
    // bounded by `d` would see only the leading [0,10] and [10,10].
    assert_eq!(n[5], 5.0, "SIZE must be the physical length: n = {n:?}");
    assert_eq!(n[6], 4.0, "n = {n:?}");
    assert!((total[5] - 25.0).abs() < EPS, "total = {total:?}");

    assert!(
        (avg[5] - 5.0).abs() < EPS,
        "MEAN must divide by the 5 physical slats, not the entry depth 2 \
         (which would give {}); got {}",
        total[5] / 2.0,
        avg[5]
    );
    for (i, ((&a, &c), &t)) in avg.iter().zip(n.iter()).zip(total.iter()).enumerate() {
        assert!(
            (a * c - t).abs() < EPS,
            "row {i}: MEAN*SIZE = {} != {t}",
            a * c
        );
    }
    // MIN over [10,10,5,5] is 5; over the two-deep head it would be 10.
    assert!(
        (lo[6] - 5.0).abs() < EPS,
        "MIN must fold the tail too: {lo:?}"
    );
    // STDDEV over [0,10,5,5,5]: mean 5, squared deviations 25+25, variance 50/5 = 10.
    assert!(
        (spread[5] - 10.0_f64.sqrt()).abs() < EPS,
        "spread = {spread:?}"
    );
    // `belt[5]` is the fifth PHYSICAL slat: live at row 5, past the end at row 6.
    assert!((far[5] - 5.0).abs() < EPS, "far_slat = {far:?}");
    assert!(
        far[6].is_nan(),
        "belt[5] over a 4-slat belt is NaN: {far:?}"
    );

    assert!(
        n.iter().any(|v| *v > 2.0) && n[n.len() - 1] == 2.0,
        "the belt must collapse onto the entry depth eventually: {n:?}"
    );
}

/// Containers on a LEAKY belt: the reducers read the slats after phase A's shed, i.e.
/// as the previous step's leak left them, and the belt is non-uniform (a full-zone
/// linear leak drains the deeper slats less).
#[test]
fn container_on_a_leaky_belt_matches_vm() {
    let xml = leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.3")],
    )
    .replace(
        "</variables>",
        r#"<aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>
           <aux name="avg"><eqn>MEAN(belt)</eqn></aux>
           </variables>"#,
    );
    let project = parse(&xml);
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let total = wasm_series(&artifact, &slab, "total");
    let belt = wasm_series(&artifact, &slab, "belt");
    for (i, (&t, &b)) in total.iter().zip(belt.iter()).enumerate() {
        assert!((t - b).abs() < EPS, "row {i}: SUM = {t}, belt = {b}");
    }
    // The leak makes the belt genuinely non-uniform, so MIN < MAX once it has filled.
    let (lo, hi) = (
        wasm_series(&artifact, &slab, "lo"),
        wasm_series(&artifact, &slab, "hi"),
    );
    let last = lo.len() - 1;
    assert!(
        lo[last] + EPS < hi[last],
        "a leaky belt's slats must differ: MIN = {}, MAX = {}",
        lo[last],
        hi[last]
    );
    let avg = wasm_series(&artifact, &slab, "avg");
    assert!(
        (avg[last] - total[last] / 4.0).abs() < EPS,
        "MEAN must divide by the four physical slats"
    );
}

/// Containers on a DISCRETE belt: the slats hold whole units and the time-unit block
/// merge lumps them, so the reducers read a coarser vector than a continuous belt's.
#[test]
fn container_on_a_discrete_belt_matches_vm() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "4",
            initial: "12",
            len: "2",
            inflow: "3.4",
            conv_attrs: r#"isee:discrete="true""#,
            conv_extra: "",
        },
        r#"<aux name="n"><eqn>SIZE(belt)</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="exit_slat"><eqn>belt[1]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let total = wasm_series(&artifact, &slab, "total");
    let belt = wasm_series(&artifact, &slab, "belt");
    for (i, (&t, &b)) in total.iter().zip(belt.iter()).enumerate() {
        assert!((t - b).abs() < EPS, "row {i}: SUM = {t}, belt = {b}");
    }
    // A discrete belt carries whole units, so every published slat volume is one.
    for (i, &v) in wasm_series(&artifact, &slab, "exit_slat")
        .iter()
        .enumerate()
    {
        assert_eq!(v.fract(), 0.0, "row {i}: discrete belt[1] = {v}");
    }
    assert_eq!(wasm_series(&artifact, &slab, "n"), vec![4.0; 9]);
}

/// Containers on an ARRESTED belt. `<arrest>` freezes the latch, the leak, and the
/// exit; phase B does nothing. So the published slat vector is FROZEN for as long as
/// the arrest holds, and resumes moving after -- which pins that the publish reads the
/// live ring rather than recomputing anything from the flows.
#[test]
fn container_on_an_arrested_belt_matches_vm() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "3",
            initial: "1, 2, 3, 4",
            len: "2",
            inflow: "0",
            conv_attrs: "",
            // `STEP(1, 1) - STEP(1, 2)` is 1 over `[1, 2)` and 0 elsewhere. Spelled with
            // STEP rather than a comparison because `&lt;` in an XMILE equation is a
            // readability trap this file has no other reason to introduce.
            conv_extra: "<arrest>STEP(1, 1) - STEP(1, 2)</arrest>",
        },
        r#"<aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="exit_slat"><eqn>belt[1]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // Published slat vectors, row by row: [1,2,3,4], [2,3,4,0], [3,4,0,0], then the
    // steps at t=1.0 and t=1.5 are arrested, so t=1.5 and t=2.0 republish [3,4,0,0]
    // unchanged. The arrest lifts at t=2 (`STEP(1, 2)` fires), so t=2.0's step moves the
    // belt again and t=2.5 publishes [4,0,0,0].
    let exit = wasm_series(&artifact, &slab, "exit_slat");
    let total = wasm_series(&artifact, &slab, "total");
    assert_eq!(
        &exit[..6],
        &[1.0, 2.0, 3.0, 3.0, 3.0, 4.0],
        "exit = {exit:?}"
    );
    assert_eq!(
        &total[..6],
        &[10.0, 9.0, 7.0, 7.0, 7.0, 4.0],
        "total = {total:?}"
    );
    // Three consecutive equal publishes are the arrest; a fourth would mean the belt
    // never restarted.
    assert_eq!(exit[6], 0.0, "the belt must resume after the arrest");
}

/// `MIN`/`MAX` fold with a `select` on a STRICT comparison, never `f64.min`/`f64.max`:
/// Rust's methods return the other operand when one is NaN, wasm's instructions
/// propagate the NaN. A leaky belt seeded AND fed an infinite volume separates them.
/// The steady fill scales every slat by `E = INF`; the first leak step computes
/// `INF - INF = NaN` on every slat that already held material, while phase B keeps
/// inserting a fresh `+INF` cohort at the entry. From row 1 on the belt is therefore a
/// MIXTURE of NaN and `+INF` slats: `SUM` is NaN, but `MIN` and `MAX` must SKIP the
/// NaNs and answer `+INF`.
///
/// This is the test that fails if [`emit_min_max`]'s `select` is replaced by `f64.min`
/// / `f64.max` -- both would answer NaN.
#[test]
fn nan_slats_fold_like_rust_not_like_wasm() {
    let xml = leaky_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "3",
            initial: "1/0",
            len: "2",
            inflow: "1/0",
            conv_attrs: "",
            conv_extra: "",
        },
        &[Leak::full("0.5")],
    )
    .replace(
        "</variables>",
        r#"<aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="spread"><eqn>STDDEV(belt)</eqn></aux>
           </variables>"#,
    );
    let project = parse(&xml);
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let lo = wasm_series(&artifact, &slab, "lo");
    let hi = wasm_series(&artifact, &slab, "hi");
    let total = wasm_series(&artifact, &slab, "total");
    let spread = wasm_series(&artifact, &slab, "spread");

    // The belt must actually carry NaN slats, or the test proves nothing about the
    // NaN discipline. `SUM` propagates -- it is a plain `f64.add` fold, exactly like
    // `iter().sum()` -- so a NaN total IS the witness.
    assert!(total[0].is_infinite(), "row 0 is all +INF: {total:?}");
    assert!(
        total[1..].iter().all(|v| v.is_nan()),
        "the leak must leave NaN slats behind: {total:?}"
    );
    // And yet MIN/MAX see through them, on every row.
    for (i, (&l, &h)) in lo.iter().zip(hi.iter()).enumerate() {
        assert!(
            l.is_infinite() && l > 0.0,
            "row {i}: MIN must skip the NaN slats and answer +INF, got {l}"
        );
        assert!(
            h.is_infinite() && h > 0.0,
            "row {i}: MAX must skip the NaN slats and answer +INF, got {h}"
        );
    }
    // STDDEV has no such skip: `INF - INF` poisons its very first deviation.
    assert!(spread.iter().all(|v| v.is_nan()), "spread = {spread:?}");
}

/// An ARRAYED conveyor is N independent belts, and its container variable is arrayed
/// over the same dims -- so `SUM(belt[a])` reduces element `a`'s OWN slats. The two
/// elements are given different transit times, so a lowering that published one belt's
/// reduction into both slots (or read the wrong descriptor) is caught.
#[test]
fn arrayed_conveyor_container_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <dimensions><dim name="board"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <stock name="belt"><dimensions><dim name="board"/></dimensions>
      <eqn>12</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>transit[board]</len></conveyor></stock>
    <aux name="transit"><dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>0.5</eqn></element>
      <element subscript="b"><eqn>2</eqn></element></aux>
    <flow name="in_f"><dimensions><dim name="board"/></dimensions><eqn>0</eqn></flow>
    <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
    <aux name="n_a"><eqn>SIZE(belt[a])</eqn></aux>
    <aux name="n_b"><eqn>SIZE(belt[b])</eqn></aux>
    <aux name="sum_a"><eqn>SUM(belt[a])</eqn></aux>
    <aux name="sum_b"><eqn>SUM(belt[b])</eqn></aux>
    <aux name="hi_b"><eqn>MAX(belt[b])</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Element `a` has one slat (transit 0.5 == dt) and dumps its 12 in one step;
    // element `b` has four, each holding 3, and drains one per DT.
    let slab = run_artifact(&artifact);
    let series = |name: &str| wasm_series(&artifact, &slab, name);
    assert_eq!(series("n_a"), vec![1.0; 5], "element a has one slat");
    assert_eq!(series("n_b"), vec![4.0; 5], "element b has four");
    assert_eq!(series("sum_a"), vec![12.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(series("sum_b"), vec![12.0, 9.0, 6.0, 3.0, 0.0]);
    assert_eq!(series("hi_b"), vec![3.0, 3.0, 3.0, 3.0, 0.0]);
}

/// The mid-run preview re-publishes containers from the REAL belt and then runs the
/// pass on a CLONE, so a mid-run `get_value` of `SUM(belt)` reads the resting
/// start-of-step value the resumed step will recompute -- and resuming still lands on
/// the byte-identical single-`run` slab.
///
/// The publish itself writes only `curr`, and `run_to`'s tail re-publishes before the
/// Flows re-eval, so it adds no state for the preview to roll back. This test is what
/// says so: a publish that wrote the CLONED ring's reduction, or one placed after the
/// preview pass, would leave the resting `SUM` one step ahead of the VM's.
///
/// The inflow RAMPS, so no two slats are equal; a steady-state belt would make the
/// preview's clone-and-restore unobservable.
#[test]
fn container_preview_is_side_effect_free() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "6",
            initial: "0",
            len: "1.5",
            inflow: "TIME",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>
           <aux name="exit_slat"><eqn>belt[1]</eqn></aux>"#,
    ));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    // The VM's own mid-run preview (cloned side tables) is the oracle for the resting
    // `curr`, not a hand-derived constant.
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("VM must build the conveyor model");
    vm.run_to(3.0).expect("VM must run to the preview point");

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 3.0);

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
        assert!(checked >= 8, "expected to compare the whole curr row");

        call_run_to(store, inst, 6.0);
        assert_eq!(get_error(store, inst), 0);
        assert_eq!(
            single,
            read_slab(store, inst, &artifact),
            "the preview double-advanced the belt"
        );
    });
}

/// A published container slot is pass-written, so `set_value` must reject it -- exactly
/// as the VM's `set_value_by_offset` does (GH #871). Every kind gets its own hidden
/// stock, so every kind's slot is checked; a genuine constant aux stays overridable, so
/// the rejection is not a blanket refusal.
#[test]
fn set_value_rejects_container_slots() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "2",
            initial: "8",
            len: "2",
            inflow: "rate",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="rate"><eqn>7</eqn></aux>
           <aux name="total"><eqn>SUM(belt)</eqn></aux>
           <aux name="avg"><eqn>MEAN(belt)</eqn></aux>
           <aux name="n"><eqn>SIZE(belt)</eqn></aux>
           <aux name="lo"><eqn>MIN(belt)</eqn></aux>
           <aux name="hi"><eqn>MAX(belt)</eqn></aux>
           <aux name="spread"><eqn>STDDEV(belt)</eqn></aux>
           <aux name="exit_slat"><eqn>belt[1]</eqn></aux>"#,
    ));
    let main = project.models[0].name.clone();
    let artifact = artifact_for(&project);

    // The HIDDEN container stocks, by their synthesized canonical names -- not the
    // reader auxes, which are ordinary variable references and would reject for the
    // uninteresting reason that they are not constants.
    let hidden = [
        "$conv$sum$belt",
        "$conv$mean$belt",
        "$conv$size$belt",
        "$conv$min$belt",
        "$conv$max$belt",
        "$conv$stddev$belt",
        "$conv$slat$belt$1",
    ];
    let mut vm = build_vm(&project, &main).expect("vm");
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
        for name in hidden {
            let off = layout_offset(&artifact, name);
            assert_eq!(try_set(store, off, 1.0), 1, "{name} is not overridable");
            assert!(
                vm.set_value_by_offset(off, 1.0).is_err(),
                "the VM must reject {name} too"
            );
        }
        let rate = layout_offset(&artifact, "rate");
        assert_eq!(
            try_set(store, rate, 3.0),
            0,
            "a constant aux stays settable"
        );
        assert!(vm.set_value_by_offset(rate, 3.0).is_ok());
    });
}

// ── what still rejects ───────────────────────────────────────────────────────

/// `reject_unsupported` now refuses exactly TWO things, and container access is no
/// longer one of them: a belt read through `SUM`/`conv[j]` must LOWER (and be pinned
/// by the parity tests above), never be refused. GH #923 deleted that arm, and this
/// test is what keeps it deleted -- a re-added reject would make every container
/// consumer silently unlowerable rather than merely wrong.
///
/// The two survivors have their own tests: `spreadflow_placements_are_rejected` (the
/// §8 feature gap, GH #946) and the `i32::MAX` slat-bound soundness guard, which no
/// test can reach without a bound above `i32::MAX` -- `SlatBoundGuard` takes a `usize`
/// and the guard is a `>` comparison, so a test would have to allocate that ring.
/// The `belt[j]` slat access exercised here also drives GH #948: it lowers and
/// simulates correctly on both backends, but the diagnostic pass still emits a
/// spurious Error for the subscript. `lower` never reads diagnostics, so this test
/// stays green either way; the shape is pinned here for whoever fixes #948.
#[test]
fn container_access_is_no_longer_rejected() {
    let project = parse(&container_belt_xmile(
        BeltXmile {
            dt: "0.5",
            stop: "2",
            initial: "0",
            len: "2",
            inflow: "10",
            conv_attrs: "",
            conv_extra: "",
        },
        r#"<aux name="tot"><eqn>SUM(belt)</eqn></aux>
           <aux name="slat"><eqn>belt[1]</eqn></aux>"#,
    ));
    lower(&project).expect("a container-reading belt must lower, not reject");
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

/// `test/conveyors/covid19_severity.stmx` carries leak flows -- now lowered -- but
/// NEITHER backend compiles it, and the reason is NOT the GH #924 wasm gate.
///
/// Its `death rate` aux is `SUM(contagious_deaths[*]) + ...`: a direct, same-step read
/// of four conveyor-driven LEAK flows, which the engine rejects with
/// `ConveyorDrivenFlowRead` because the belt pass runs between the Flows and Stocks
/// phases, so the flows-phase read would see the placeholder `0`. Real Stella models
/// use exactly this idiom to report a belt's leak rates, so the rejection blocks a
/// legitimate model rather than an edge case -- tracked as GH #944.
///
/// The error code is asserted on BOTH backends, not just the `Err`: this fixture is one
/// of the corpus-gate candidates, and a bare `is_err()` would keep passing if the fixture
/// later failed for some unrelated reason -- including the reason this whole file exists
/// to rule out, a blanket wasm-side conveyor reject. See [`assert_shared_dispatch_refusal`].
///
/// Its sibling `sir_social_distancing_mixnot.stmx` is blocked for a different reason
/// again; see [`sir_fixture_is_blocked_by_the_submodel_rule`].
#[test]
fn covid_leak_fixture_is_blocked_by_the_driven_flow_read_rule() {
    let project = parse(include_str!(
        "../../../../test/conveyors/covid19_severity.stmx"
    ));
    assert_shared_dispatch_refusal(&project, ErrorCode::ConveyorDrivenFlowRead);
}

/// Assert that BOTH backends refuse `project` for the same shared-dispatch reason,
/// identified by `code`.
///
/// The wasm side cannot compare an `ErrorCode` directly -- `compile_datamodel_to_artifact`
/// wraps a `queue_compile::compile_sim` failure as `Unsupported(format!("wasmgen:
/// incremental compile failed: {{e:?}}"))` -- but `Error`'s derived `Debug` carries the
/// variant name, so the code is recoverable from the message. Matching on it is what
/// distinguishes "the shared dispatch refuses this real model" from "the wasm backend has
/// a conveyor gap": a blanket reject on the public entry satisfies `is_err()` and would
/// leave a bare-`is_err()` assertion green.
fn assert_shared_dispatch_refusal(project: &crate::datamodel::Project, code: ErrorCode) {
    let main = project.models[0].name.clone();
    let err = build_vm(project, &main).expect_err("the VM must refuse this fixture");
    assert_eq!(
        err.code, code,
        "the fixture must stay blocked for the reason its issue describes: {err:?}"
    );
    // `WasmArtifact` is not `Debug`, so `expect_err` is unavailable; destructure instead.
    let Err(WasmGenError::Unsupported(msg)) = lower(project) else {
        panic!("the wasm entry must refuse this fixture too");
    };
    assert!(
        msg.contains(&format!("{code:?}")),
        "the wasm entry routes through the same compile_sim, so it must refuse for the \
         SAME reason ({code:?}) -- not because the backend rejects conveyors; got: {msg}"
    );
}

/// The other checked-in conveyor fixture, `sir_social_distancing_mixnot.stmx`, is
/// blocked at an earlier gate still: its conveyor lives in a SUB-MODEL, which
/// expansion never descends into, so `compile_sim` refuses it for BOTH backends
/// (GH #941). It is a discrete belt with a `dist` spread inflow besides, so it stays
/// out of wasm scope even once #941 is resolved.
///
/// Pinned on the error CODE for the same reason as its sibling: the corpus gate runs
/// every OTHER `test/conveyors/` fixture, and must not read "the engine refuses a real
/// model" as "expected reject". Between them these two tests establish that neither
/// VENDORED conveyor fixture is a usable parity oracle today -- a fact that is otherwise
/// invisible.
#[test]
fn sir_fixture_is_blocked_by_the_submodel_rule() {
    let project = parse(include_str!(
        "../../../../test/conveyors/sir_social_distancing_mixnot.stmx"
    ));
    assert_shared_dispatch_refusal(&project, ErrorCode::ConveyorInSubmodelUnsupported);
}

/// The inverse of the reject this suite used to pin (GH #924): the PUBLIC entry point
/// -- the one `libsimlin`'s `simlin_model_compile_to_wasm` calls -- lowers a conveyor
/// model, and the resulting blob reproduces the VM. Every other test in this file goes
/// through the same entry via [`lower`]; this one names it explicitly so the contract
/// is greppable from the reject it replaced.
#[test]
fn public_entry_lowers_conveyor_models() {
    let project = parse(&one_belt_xmile("0.5", "2", "0", "1", "10", ""));
    let main = project.models[0].name.clone();
    let artifact = crate::wasmgen::compile_datamodel_to_artifact(&project, &main, false, false)
        .expect("the public entry must lower a conveyor model");
    assert!(assert_slab_matches_vm(&project, &artifact) > 0);
}
