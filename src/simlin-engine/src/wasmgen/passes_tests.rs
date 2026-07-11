// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the special-stock side-table pass lowering ([`super`]). Split out
//! of `passes.rs` to keep that file under the project line-count lint; this is
//! the `#[cfg(test)] mod tests` body, included via `#[path]` so `use super::*`
//! still resolves the module's private items.
//!
//! The bytecode VM is the oracle: every test compiles the SAME datamodel through
//! `queue_compile::build_vm` (the VM's special-stock build path) and through
//! `wasmgen::compile_datamodel_to_artifact` (which routes through the same
//! `queue_compile::compile_sim` dispatch), runs the blob under the DLR-FT
//! interpreter, and diffs the two slabs variable-by-variable.

use super::*;

use crate::common::{Canonical, Ident};
use crate::queue_compile::{QueueOutflowPlan, build_vm};
use crate::wasmgen::{WasmArtifact, WasmGenError, compile_datamodel_to_artifact};
use checked::{Store, Stored};
use std::io::BufReader;
use wasm::addrs::ModuleAddr;
use wasm::validate;

/// The DLR-FT store + the instantiated module handle, threaded through the driver
/// helpers below.
type TestStore<'a> = Store<'a, ()>;
type Inst = Stored<ModuleAddr>;

/// A VM-vs-wasm mismatch above this is a failure. Both backends run the identical
/// opcode program over the identical slab, and the pass itself is a
/// transcription -- the only slack is the pass's own accumulation order, which is
/// identical too. Anything looser would hide a real divergence.
const EPS: f64 = 1e-9;

fn parse(xml: &str) -> crate::datamodel::Project {
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("parse xmile")
}

fn artifact_for(project: &crate::datamodel::Project) -> WasmArtifact {
    let main = project.models[0].name.clone();
    compile_datamodel_to_artifact(project, &main, false, false)
        .expect("a queue model must lower to wasm")
}

/// Instantiate a blob under the interpreter and drive it with `body`.
///
/// A closure rather than a `(store, inst)` return pair because `Store<'b, _>`
/// borrows the `ValidationInfo` it was instantiated from, so the two cannot leave
/// this frame together.
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

/// Copy the whole step-major results slab out of the instance's linear memory.
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

/// Linear memory's current byte length -- the observable the leak tests watch
/// across repeated `reset`/`run` cycles.
fn memory_bytes(store: &mut TestStore<'_>, inst: Inst) -> usize {
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export")
        .as_mem()
        .expect("memory is a memory");
    store.mem_access_mut_slice(mem, |bytes| bytes.len())
}

/// Run a blob's single-shot `run` and return the results slab.
fn run_artifact(artifact: &WasmArtifact) -> Vec<f64> {
    with_instance(artifact, |store, inst| {
        call_void(store, inst, "run");
        read_slab(store, inst, artifact)
    })
}

/// Run `project` through the bytecode VM's special-stock build path.
fn vm_slab(project: &crate::datamodel::Project) -> crate::Results {
    let main = project.models[0].name.clone();
    let mut vm = build_vm(project, &main).expect("VM must build the queue model");
    vm.run_to_end().expect("VM must run the queue model");
    vm.into_results()
}

/// Assert every variable in `artifact.layout` matches the VM's series, NaN-aware
/// (an empty-queue `MEAN`/`MIN`/`MAX`/`STDDEV` and an out-of-range `queue[k]` are
/// genuinely NaN on both sides). Returns the number of variables compared.
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

/// The layout's slot offset for `name`.
fn layout_offset(artifact: &WasmArtifact, name: &str) -> usize {
    artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name} in layout"))
        .1
}

/// The series of `name` from a wasm results slab.
fn wasm_series(artifact: &WasmArtifact, slab: &[f64], name: &str) -> Vec<f64> {
    let off = layout_offset(artifact, name);
    (0..artifact.layout.n_chunks)
        .map(|c| slab[c * artifact.layout.n_slots + off])
        .collect()
}

// ── the checked-in fixtures ──────────────────────────────────────────────────

/// The `test/queues/queue_drain.xmile` fixture: a pass-through queue whose single
/// unconstrained outflow empties it every step. Nothing under `test/` changed to
/// make this run; the fixture previously never reached the wasm backend at all
/// (`compile_datamodel_to_artifact` rejected every queue model up front).
#[test]
fn queue_drain_fixture_matches_vm() {
    let project = parse(include_str!("../../../../test/queues/queue_drain.xmile"));
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 4, "expected the whole model, checked {checked}");

    // Pin that the pass actually ran: the driven outflow carries the served rate
    // (the constant inflow, 10) at EVERY row, not the placeholder 0 its equation
    // compiles to. The queue starts empty and admits-then-serves within one step,
    // so even row 0 is 10.
    let slab = run_artifact(&artifact);
    let served = wasm_series(&artifact, &slab, "into_service");
    assert!(
        served.iter().all(|v| (v - 10.0).abs() < EPS),
        "into_service should be the served rate 10, got {served:?}"
    );
}

/// The `test/queues/minimal_queue.xmile` fixture: two outflows, the second marked
/// `<overflow/>`. Behind an UNCONSTRAINED primary the redirectable budget is 0
/// (the primary was never blocked), so the overflow must drain nothing -- the
/// `queues.md` §4.5 no-upstream-conveyor behavior.
#[test]
fn minimal_queue_overflow_fixture_matches_vm() {
    let project = parse(include_str!("../../../../test/queues/minimal_queue.xmile"));
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let balk = wasm_series(&artifact, &slab, "balk");
    assert!(
        balk.iter().all(|v| *v == 0.0),
        "an overflow behind an unconstrained primary drains nothing, got {balk:?}"
    );
}

// ── the pass's behavioral surface ────────────────────────────────────────────

/// A queue seeded with a positive initial value starts with one front batch
/// (`queue.rs:84`); the pass-through outflow drains it in the first step.
#[test]
fn queue_initial_value_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>17</eqn><inflow>arrivals</inflow><outflow>leaving</outflow><queue/></stock>
    <flow name="arrivals"><eqn>3</eqn><non_negative/></flow>
    <flow name="leaving"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>leaving</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Row 0 records `curr` AFTER the t=0 step's pass, so it already carries the
    // t=0 serve: the seeded batch (17) plus the just-admitted 3*0.5, drained at
    // (17 + 1.5)/0.5 = 37. Every later step drains only the admitted 1.5, i.e.
    // rate 3. A blob that never seeded the FIFO would report 3 at row 0 too.
    let slab = run_artifact(&artifact);
    let leaving = wasm_series(&artifact, &slab, "leaving");
    assert!(
        (leaving[0] - 37.0).abs() < EPS,
        "leaving[0] = {}",
        leaving[0]
    );
    assert!(
        (leaving[1] - 3.0).abs() < EPS,
        "leaving[1] = {}",
        leaving[1]
    );
    // And the queue stock's own series starts at its seeded 17.
    let waiting = wasm_series(&artifact, &slab, "waiting");
    assert!(
        (waiting[0] - 17.0).abs() < EPS,
        "waiting[0] = {}",
        waiting[0]
    );
}

/// A negative (and a NaN) inflow contributes no batch AND is clamped in its own
/// slot, so the ordinary Stocks phase integrates the same volume the FIFO
/// admitted (`queue_compile.rs:703`, the §4.1 conservation identity).
///
/// This is the test that would fail if the blob used wasm's `f64.max` -- which
/// propagates NaN -- instead of the `select` form Rust's `f64::max` implements.
#[test]
fn nonpositive_and_nan_inflows_clamp_in_place_like_vm() {
    // `swing` alternates sign; `poison` is 0/0 = NaN for t >= 2.
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>swing</inflow><inflow>poison</inflow><outflow>leaving</outflow><queue/></stock>
    <flow name="swing"><eqn>IF TIME MOD 2 = 0 THEN 6 ELSE -4</eqn></flow>
    <flow name="poison"><eqn>IF TIME &gt;= 2 THEN 0/0 ELSE 1</eqn></flow>
    <flow name="leaving"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>leaving</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // The clamp is written BACK into the flow slot, so the negative and NaN
    // inflows publish 0 rather than -4 / NaN.
    let swing = wasm_series(&artifact, &slab, "swing");
    let poison = wasm_series(&artifact, &slab, "poison");
    assert!(
        swing.iter().all(|v| *v >= 0.0),
        "a negative inflow must be clamped in place, got {swing:?}"
    );
    assert!(
        poison.iter().all(|v| !v.is_nan()),
        "a NaN inflow must clamp to 0 (Rust f64::max), not propagate, got {poison:?}"
    );
    // Conservation: served == the sum of the clamped inflow volumes.
    let served = wasm_series(&artifact, &slab, "served");
    let expected: f64 = swing
        .iter()
        .zip(&poison)
        .take(served.len() - 1)
        .map(|(a, b)| a + b)
        .sum();
    assert!(
        (served.last().unwrap() - expected).abs() < EPS,
        "served {} != Σ clamped inflows {expected}",
        served.last().unwrap()
    );
}

/// Two independent queues in one model each get their own descriptor + ring, and
/// the unrolled pass drives both.
#[test]
fn two_queues_match_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>5</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="qa"><eqn>2</eqn><inflow>in_a</inflow><outflow>out_a</outflow><queue/></stock>
    <flow name="in_a"><eqn>4</eqn><non_negative/></flow>
    <flow name="out_a"><eqn>0</eqn></flow>
    <stock name="qb"><eqn>0</eqn><inflow>out_a</inflow><outflow>out_b</outflow><queue/></stock>
    <flow name="out_b"><eqn>0</eqn></flow>
    <stock name="sink"><eqn>0</eqn><inflow>out_b</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 6, "expected both queues, checked {checked}");
}

/// An ARRAYED queue is N independent FIFOs (`queues.md` §6), flattened by
/// `resolve_plans` into one `QueuePlan` per element -- so the pass emitter sees N
/// scalar plans and unrolls N descriptors.
#[test]
fn arrayed_queue_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
  <dimensions><dim name="Line"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <stock name="waiting">
      <dimensions><dim name="Line"/></dimensions>
      <eqn>0</eqn><inflow>arrivals</inflow><outflow>leaving</outflow><queue/>
    </stock>
    <flow name="arrivals">
      <dimensions><dim name="Line"/></dimensions>
      <element subscript="a"><eqn>10</eqn></element>
      <element subscript="b"><eqn>25</eqn></element>
      <non_negative/>
    </flow>
    <flow name="leaving"><dimensions><dim name="Line"/></dimensions><eqn>0</eqn></flow>
    <stock name="served"><dimensions><dim name="Line"/></dimensions><eqn>0</eqn><inflow>leaving</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    let checked = assert_slab_matches_vm(&project, &artifact);
    assert!(checked >= 6, "expected both elements, checked {checked}");
}

// ── container access (the step-start hook point) ─────────────────────────────

/// A pure-accumulator queue (inflow, NO outflow) read through every container
/// access form. This is the twin of `queue_compile`'s
/// `scalar_queue_container_size_sum_and_reducers`, run through the wasm backend:
/// it covers `SIZE`/`SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`, an in-range `queue[k]` and
/// an out-of-range one (NaN), and `INIT(SUM(queue))` (the reconciliation pass).
///
/// With `dt = 1` and five steps the FIFO grows to five batches, past the initial
/// ring capacity -- so this also exercises `q_grow` and, on a tight layout,
/// `memory.grow`.
#[test]
fn queue_container_access_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10 * (TIME + 1)</eqn><non_negative/></flow>
    <aux name="n"><eqn>SIZE(waiting)</eqn></aux>
    <aux name="total"><eqn>SUM(waiting)</eqn></aux>
    <aux name="avg"><eqn>MEAN(waiting)</eqn></aux>
    <aux name="lo"><eqn>MIN(waiting)</eqn></aux>
    <aux name="hi"><eqn>MAX(waiting)</eqn></aux>
    <aux name="spread"><eqn>STDDEV(waiting)</eqn></aux>
    <aux name="front"><eqn>waiting[1]</eqn></aux>
    <aux name="beyond"><eqn>waiting[9]</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    // Independent oracle, so this cannot pass vacuously if BOTH backends were
    // wrong: start-of-step batch vectors are [], [10], [10,20], ...
    let slab = run_artifact(&artifact);
    assert_eq!(
        wasm_series(&artifact, &slab, "n"),
        vec![0.0, 1.0, 2.0, 3.0, 4.0]
    );
    assert_eq!(
        wasm_series(&artifact, &slab, "total"),
        vec![0.0, 10.0, 30.0, 60.0, 100.0]
    );
    let avg = wasm_series(&artifact, &slab, "avg");
    assert!(avg[0].is_nan(), "MEAN of an empty queue is NaN");
    assert_eq!(&avg[1..], &[10.0, 15.0, 20.0, 25.0]);
    let spread = wasm_series(&artifact, &slab, "spread");
    assert!(spread[0].is_nan());
    assert!(
        (spread[4] - 125.0_f64.sqrt()).abs() < EPS,
        "STDDEV {spread:?}"
    );
    let beyond = wasm_series(&artifact, &slab, "beyond");
    assert!(
        beyond.iter().all(|v| v.is_nan()),
        "an out-of-range batch index is NaN, got {beyond:?}"
    );
}

/// `INIT(<container access>)` must read the START-OF-RUN batch total, not the
/// container stock's frozen `0` placeholder. This is the wasm twin of
/// `queue_compile`'s `queue_container_init_reads_start_of_run_not_placeholder`,
/// and it is what the `run_initials` reconciliation (a second, container-skipping
/// initials pass plus a re-snapshot) exists for.
#[test]
fn queue_container_init_reconciles_like_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>40</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <aux name="init_sum"><eqn>INIT(SUM(waiting))</eqn></aux>
    <aux name="init_size"><eqn>INIT(SIZE(waiting))</eqn></aux>
    <aux name="ratio"><eqn>SUM(waiting) / INIT(SUM(waiting))</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    for (i, &v) in wasm_series(&artifact, &slab, "init_sum").iter().enumerate() {
        assert!((v - 40.0).abs() < EPS, "step {i}: INIT(SUM) = {v}, want 40");
    }
    for (i, &v) in wasm_series(&artifact, &slab, "init_size")
        .iter()
        .enumerate()
    {
        assert!((v - 1.0).abs() < EPS, "step {i}: INIT(SIZE) = {v}, want 1");
    }
    // Without the reconciliation this would be inf (a division by the frozen 0).
    let ratio = wasm_series(&artifact, &slab, "ratio");
    assert!(
        ratio.iter().all(|v| v.is_finite()),
        "INIT(SUM(queue)) must not be the frozen 0 placeholder, got {ratio:?}"
    );
}

/// The step-start publish and the between-Flows-and-Stocks pass are DISTINCT hook
/// points: a Flows-phase reader of `SUM(queue)` sees the batches as the PREVIOUS
/// step's admit/serve left them, never this step's post-serve state. A drained
/// pass-through queue therefore reports `SUM == 0` at every step even though it
/// admits (and immediately serves) 10 per step.
#[test]
fn container_publish_is_start_of_step_not_post_serve() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>5</eqn><inflow>arrivals</inflow><outflow>leaving</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="leaving"><eqn>0</eqn></flow>
    <aux name="backlog"><eqn>SUM(waiting)</eqn></aux>
    <aux name="n"><eqn>SIZE(waiting)</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    // t=0 sees the seeded batch (5); every later step sees an emptied FIFO,
    // because the previous step's unconstrained primary drained it. If the two
    // hook points were collapsed into one (publish after the pass), every step
    // would read 0 -- including t=0.
    let backlog = wasm_series(&artifact, &slab, "backlog");
    assert_eq!(backlog[0], 5.0, "t=0 sees the seeded batch");
    assert!(
        backlog[1..].iter().all(|v| *v == 0.0),
        "a drained pass-through queue is empty at every later step start, got {backlog:?}"
    );
    let n = wasm_series(&artifact, &slab, "n");
    assert_eq!(n[0], 1.0, "the seeded FIFO holds exactly one batch");
}

// ── lifecycle: memory, reset, resume ─────────────────────────────────────────

/// A queue whose ring outgrows its initial capacity several times over: the
/// doubling path (`q_grow`) and the bump allocator's `memory.grow` both run, and
/// the batch vector stays in FIFO order (which `SUM`/`MIN`/`MAX`/`waiting[1]`
/// would all betray if the ring-order copy were wrong).
#[test]
fn ring_growth_past_initial_capacity_matches_vm() {
    // dt = 0.25 over 0..8 admits 33 batches into a queue with no outflow -- four
    // doublings past the 4-slot initial ring.
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>8</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>TIME + 1</eqn><non_negative/></flow>
    <aux name="n"><eqn>SIZE(waiting)</eqn></aux>
    <aux name="total"><eqn>SUM(waiting)</eqn></aux>
    <aux name="oldest"><eqn>waiting[1]</eqn></aux>
    <aux name="lo"><eqn>MIN(waiting)</eqn></aux>
    <aux name="hi"><eqn>MAX(waiting)</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    assert_slab_matches_vm(&project, &artifact);

    let slab = run_artifact(&artifact);
    let n = wasm_series(&artifact, &slab, "n");
    assert!(
        *n.last().unwrap() >= 32.0,
        "the ring must have grown well past its 4-slot start, final SIZE = {}",
        n.last().unwrap()
    );
    // The oldest batch stays at the front across every doubling: the first admit
    // is `(TIME=0) + 1` times `dt` = 0.25.
    let oldest = wasm_series(&artifact, &slab, "oldest");
    assert!(
        oldest[1..].iter().all(|v| (v - 0.25).abs() < EPS),
        "ring order lost across a doubling, waiting[1] = {oldest:?}"
    );
}

/// `reset` rewinds the bump pointer, so repeated `run`s (which delegate `reset;
/// run_to(stop)`) reproduce the identical slab AND settle on a fixed memory
/// footprint -- the ring capacity a run's doubling reached is reclaimed wholesale
/// rather than leaking one abandoned region per run.
#[test]
fn repeated_runs_are_identical_and_do_not_leak() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>20</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>1</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>TIME + 1</eqn><non_negative/></flow>
    <aux name="total"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    with_instance(&artifact, |store, inst| {
        let mut prev_slab: Option<Vec<f64>> = None;
        let mut prev_bytes: Option<usize> = None;
        for round in 0..4 {
            call_void(store, inst, "run");
            let slab = read_slab(store, inst, &artifact);
            let bytes = memory_bytes(store, inst);
            if let Some(p) = &prev_slab {
                assert_eq!(p, &slab, "run {round} diverged from the first run");
            }
            // Round 1 may still grow (the first run's doubling can cross a page);
            // from round 2 on the footprint must be stable, or the bump pointer is
            // not being rewound.
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

/// A RESUMED `run_to` must not re-initialize the side table. `G_DID_INITIALS`
/// already gives this: `run_initials` short-circuits on the second call, so the
/// FIFO carries its batches across the segment boundary.
///
/// The proof is behavioral, not structural: the model accumulates batches (no
/// outflow), so a re-init at the resume boundary would reset `SIZE(waiting)` to
/// its start-of-run value and the two slabs would differ. Comparing a segmented
/// drive against a single `run` -- and against the VM -- pins it.
#[test]
fn resumed_run_to_does_not_reinitialize_the_side_table() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>6</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>3</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>2</eqn><non_negative/></flow>
    <aux name="n"><eqn>SIZE(waiting)</eqn></aux>
    <aux name="total"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    // Drive the resumable ABI across three segments, each resuming mid-run.
    let segmented = with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        for target in [1.5f64, 4.0, 6.0] {
            call_run_to(store, inst, target);
        }
        read_slab(store, inst, &artifact)
    });
    assert_eq!(
        single, segmented,
        "a segmented run must land on the byte-identical slab"
    );

    // And the accumulation really is monotone, so a re-init would have shown up.
    let n = wasm_series(&artifact, &segmented, "n");
    assert!(
        n.windows(2).all(|w| w[1] >= w[0]) && *n.last().unwrap() > n[0],
        "SIZE(waiting) must accumulate across the resume boundaries, got {n:?}"
    );
    assert_slab_matches_vm(&project, &artifact);
}

/// A partial `run_to` leaves the resting `curr` chunk self-consistent: the
/// pass-driven outflow slot holds the served rate the resumed step will recompute,
/// not the placeholder `0` a bare Flows re-eval would stamp there. The preview
/// runs on a CLONED ring, so resuming still lands on the single-run slab (asserted
/// above) -- here we check the resting values themselves.
#[test]
fn midrun_resting_curr_holds_the_pass_driven_rate() {
    let project = parse(include_str!("../../../../test/queues/queue_drain.xmile"));
    let artifact = artifact_for(&project);
    let single = run_artifact(&artifact);

    with_instance(&artifact, |store, inst| {
        call_void(store, inst, "run_initials");
        call_run_to(store, inst, 2.0);

        let mem = store
            .instance_export(inst, "memory")
            .expect("memory")
            .as_mem()
            .expect("memory");
        let off = layout_offset(&artifact, "into_service");
        let served = store.mem_access_mut_slice(mem, |bytes| {
            let a = off * 8; // the live `curr` chunk starts at byte 0
            f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
        });
        assert!(
            (served - 10.0).abs() < EPS,
            "the resting curr must hold the served rate 10, got {served}"
        );

        // Resuming to the end still lands on the single-`run` slab, so the preview
        // really did leave the live side table untouched.
        call_run_to(store, inst, 4.0);
        let resumed = read_slab(store, inst, &artifact);
        assert_eq!(single, resumed);
    });
}

// ── set_value: pass-written slots are not overridable (GH #871) ──────────────

/// The blob's `set_value` must reject exactly the offsets the VM's
/// `set_value_by_offset` does. A pass-written slot -- a driven outflow rate or a
/// published container value -- compiles to a placeholder `AssignConstCurr 0`, so
/// the naive overridable-constant scan would accept it; the pass overwrites it
/// every step, making an accepted override silently ineffective.
#[test]
fn set_value_rejects_pass_written_slots() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>leaving</outflow><queue/></stock>
    <flow name="arrivals"><eqn>rate</eqn><non_negative/></flow>
    <flow name="leaving"><eqn>0</eqn></flow>
    <aux name="rate"><eqn>7</eqn></aux>
    <aux name="backlog"><eqn>SUM(waiting)</eqn></aux>
    <stock name="served"><eqn>0</eqn><inflow>leaving</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();
    let artifact = artifact_for(&project);

    let leaving = layout_offset(&artifact, "leaving");
    let backlog = layout_offset(&artifact, "backlog");
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
        // The driven outflow's slot: rejected.
        assert_eq!(
            try_set(store, leaving, 1.0),
            1,
            "a driven outflow is not overridable"
        );
        // The hidden container stock's slot: rejected.
        assert_eq!(
            try_set(store, backlog, 1.0),
            1,
            "a container slot is not overridable"
        );
        // A genuine constant aux: accepted.
        assert_eq!(
            try_set(store, rate, 3.0),
            0,
            "a constant aux stays overridable"
        );
    });

    // And the VM agrees offset-for-offset, which is the property that matters --
    // an override the blob accepts and the VM rejects (or vice versa) is the bug.
    let mut vm = build_vm(&project, &main).expect("vm");
    assert!(vm.set_value_by_offset(leaving, 1.0).is_err());
    assert!(vm.set_value_by_offset(backlog, 1.0).is_err());
    assert!(vm.set_value_by_offset(rate, 3.0).is_ok());
}

/// An accepted override on a constant that feeds a queue inflow propagates through
/// the pass exactly as it does in the VM.
#[test]
fn set_value_override_on_a_queue_inflow_matches_vm() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>leaving</outflow><queue/></stock>
    <flow name="arrivals"><eqn>rate</eqn><non_negative/></flow>
    <flow name="leaving"><eqn>0</eqn></flow>
    <aux name="rate"><eqn>7</eqn></aux>
    <stock name="served"><eqn>0</eqn><inflow>leaving</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();
    let artifact = artifact_for(&project);
    let rate_off = layout_offset(&artifact, "rate");

    let wasm_slab = with_instance(&artifact, |store, inst| {
        let set_value = store
            .instance_export(inst, "set_value")
            .expect("set_value")
            .as_func()
            .expect("func");
        let rc: i32 = store
            .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, 3.0))
            .expect("set_value invoke");
        assert_eq!(rc, 0);
        call_void(store, inst, "run");
        read_slab(store, inst, &artifact)
    });

    let mut vm = build_vm(&project, &main).expect("vm");
    vm.set_value_by_offset(rate_off, 3.0).expect("vm override");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();

    for (name, wasm_off) in &artifact.layout.var_offsets {
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..artifact.layout.n_chunks {
            let v = vm_results.data[c * vm_results.step_size + vm_off];
            let w = wasm_slab[c * artifact.layout.n_slots + *wasm_off];
            assert!((v - w).abs() < EPS, "{name} at chunk {c}: vm={v} wasm={w}");
        }
    }
    // The override took: served accumulates 3/step over 4 steps.
    let served = wasm_series(&artifact, &wasm_slab, "served");
    assert!((served.last().unwrap() - 12.0).abs() < EPS);
}

// ── what still rejects ───────────────────────────────────────────────────────

/// A CONVEYOR model is still rejected up front (GH #884, phases 2-4). The message
/// must state the wasm-backend limitation, never the engine-internal
/// `ConveyorNotExpanded` guard text (which points at a VM-only build entry point
/// that produces no blob).
#[test]
fn conveyor_models_still_rejected_up_front() {
    let project = parse(include_str!(
        "../../../../test/conveyors/minimal_conveyor.xmile"
    ));
    let main = project.models[0].name.clone();
    for ltm_enabled in [false, true] {
        match compile_datamodel_to_artifact(&project, &main, ltm_enabled, false) {
            Ok(_) => panic!("a conveyor model must not lower to wasm (ltm={ltm_enabled})"),
            Err(WasmGenError::Unsupported(msg)) => {
                assert!(
                    msg.contains("not yet supported by the wasm backend")
                        && msg.contains("conveyor"),
                    "the error must state the wasm-backend conveyor limitation, got: {msg}"
                );
                assert!(
                    !msg.contains("build_vm") && !msg.contains("build_sim"),
                    "must not direct the caller at a VM-only build entry point: {msg}"
                );
            }
        }
    }
}

/// A queue whose primary outflow feeds a discrete conveyor is served by the
/// INTERLEAVED `run_coupled_passes`, not the uncoupled admit-then-serve this
/// module emits. Emitting the uncoupled form would double-admit and mis-account,
/// so the guard is loud. (Unreachable through `compile_datamodel_to_artifact`
/// today -- a coupling needs a conveyor, and conveyor models reject above -- which
/// is exactly why it is pinned directly here.)
#[test]
fn coupled_queue_outflow_is_rejected() {
    use crate::queue_compile::{QueueOutflowKind, QueuePlan};

    let coupled = QueuePlan {
        stock_off: 4,
        inflow_offs: vec![5],
        outflows: vec![QueueOutflowPlan {
            flow_off: 6,
            kind: QueueOutflowKind::Coupled {
                conveyor: 0,
                one_at_a_time: true,
                batch_integrity: false,
            },
            overflow: false,
        }],
        containers: Vec::new(),
    };
    let err = reject_coupled_outflows(std::slice::from_ref(&coupled))
        .expect_err("a coupled queue outflow must be rejected");
    let WasmGenError::Unsupported(msg) = err;
    assert!(
        msg.contains("coupled") && msg.contains("conveyor"),
        "the error must name the coupling, got: {msg}"
    );

    // The uncoupled twin of the same plan is accepted, so the guard keys on the
    // outflow KIND rather than rejecting every plan.
    let uncoupled = QueuePlan {
        outflows: vec![QueueOutflowPlan {
            flow_off: 6,
            kind: QueueOutflowKind::Unconstrained,
            overflow: false,
        }],
        ..coupled
    };
    assert!(reject_coupled_outflows(std::slice::from_ref(&uncoupled)).is_ok());
}

/// An ordinary (queue-free) model still lowers through the same entry point and
/// carries no pass machinery -- the routing change is transparent to it.
#[test]
fn ordinary_model_still_lowers_through_the_dispatch() {
    let datamodel = crate::test_common::TestProject::new("plain")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "rate", None)
        .build_datamodel();
    let artifact = compile_datamodel_to_artifact(&datamodel, "main", false, false)
        .expect("an ordinary model must lower");
    let slab = run_artifact(&artifact);
    let level = wasm_series(&artifact, &slab, "level");
    assert_eq!(level, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
}
