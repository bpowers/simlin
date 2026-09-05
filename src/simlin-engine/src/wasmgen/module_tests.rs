// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for whole-module wasm assembly ([`super`]). Split out of
//! `module.rs` to keep that file under the project line-count lint; this is
//! the `#[cfg(test)] mod tests` body, included via `#[path]` so
//! `use super::*` still resolves the module's private items.

use super::*;
use crate::common::{Canonical, Ident};
use crate::compat::open_xmile;
use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use crate::vm::Vm;
use checked::Store;
use std::io::BufReader;
use wasm::validate;

const POPULATION_XMILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../default_projects/population/model.xmile"
);

/// A graphical function whose table is `knots`. `Continuous` kind, with the
/// x-scale spanning the knots' x-range.
fn gf_from_knots(knots: &[(f64, f64)]) -> crate::datamodel::GraphicalFunction {
    use crate::datamodel;
    let x_points: Vec<f64> = knots.iter().map(|&(x, _)| x).collect();
    let y_points: Vec<f64> = knots.iter().map(|&(_, y)| y).collect();
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(x_points.clone()),
        y_points,
        x_scale: datamodel::GraphicalFunctionScale {
            min: x_points.first().copied().unwrap_or(0.0),
            max: x_points.last().copied().unwrap_or(1.0),
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

/// Decode a GF directory's `n`th entry from `directory` bytes: the absolute
/// data byte offset and the point count.
fn decode_dir_entry(directory: &[u8], n: usize) -> (usize, usize) {
    let base = n * GF_DIRECTORY_ENTRY_BYTES as usize;
    let data_off = i32::from_le_bytes(directory[base..base + 4].try_into().unwrap()) as usize;
    let n_points = i32::from_le_bytes(directory[base + 4..base + 8].try_into().unwrap()) as usize;
    (data_off, n_points)
}

/// Decode the `(x, y)` knots stored at relative `data` offset `rel_off` for
/// a table of `n_points` (interleaved f64 LE x,y pairs).
fn decode_knots(data: &[u8], rel_off: usize, n_points: usize) -> Vec<(f64, f64)> {
    (0..n_points)
        .map(|k| {
            let a = rel_off + k * GF_KNOT_BYTES as usize;
            let x = f64::from_le_bytes(data[a..a + 8].try_into().unwrap());
            let y = f64::from_le_bytes(data[a + 8..a + 16].try_into().unwrap());
            (x, y)
        })
        .collect()
}

/// Task 1 (pure layout): `build_gf_regions` concatenates several tables into
/// the data region in order, and the directory maps each global table index
/// to its *absolute* data byte offset + point count. The data offset for
/// table `t` must be `data_base` plus the byte span of all earlier tables.
#[test]
fn build_gf_regions_lays_out_directory_and_data() {
    let region_base = 4096u32;
    let tables = vec![
        vec![(0.0, 10.0), (1.0, 20.0), (2.5, 5.0)],
        vec![(-1.0, 0.5)],
        vec![(0.0, 0.0), (10.0, 100.0)],
    ];
    let regions = build_gf_regions(&tables, region_base)
        .expect("layout must succeed")
        .expect("non-empty tables yield Some");

    // Directory immediately at region_base; data follows the directory.
    assert_eq!(regions.directory_base, region_base);
    let directory_bytes = tables.len() as u32 * GF_DIRECTORY_ENTRY_BYTES;
    assert_eq!(regions.data_base, region_base + directory_bytes);
    assert_eq!(regions.directory.len(), directory_bytes as usize);

    // Walk the directory; each table's data offset is absolute and its
    // knots round-trip exactly. The running expected offset is data_base
    // plus the byte span of all previously-laid tables.
    let mut expected_abs = regions.data_base as usize;
    let mut total_knot_bytes = 0usize;
    for (t, table) in tables.iter().enumerate() {
        let (data_off, n_points) = decode_dir_entry(&regions.directory, t);
        assert_eq!(n_points, table.len(), "table {t} point count");
        assert_eq!(data_off, expected_abs, "table {t} absolute data offset");

        let rel = data_off - regions.data_base as usize;
        assert_eq!(
            decode_knots(&regions.data, rel, n_points).as_slice(),
            table.as_slice(),
            "table {t} knots round-trip"
        );

        let span = table.len() * GF_KNOT_BYTES as usize;
        expected_abs += span;
        total_knot_bytes += span;
    }
    assert_eq!(
        regions.total_bytes as usize,
        directory_bytes as usize + total_knot_bytes,
        "total span covers directory + all knots"
    );
}

/// Task 3 (pure serializer): a `WasmLayout` round-trips through
/// `serialize`/`deserialize` -- the geometry and the full name->offset map are
/// recovered exactly. The GF offsets are not part of the wire format (a host
/// reads results by name), so they come back as 0.
#[test]
fn wasm_layout_serialize_round_trips() {
    let layout = WasmLayout {
        n_slots: 7,
        n_chunks: 101,
        results_offset: 112,
        gf_directory_offset: 4096,
        gf_data_offset: 4104,
        var_offsets: vec![
            ("time".to_string(), 0),
            ("population".to_string(), 4),
            ("a_var_with_a_longer_name".to_string(), 6),
        ],
    };
    let bytes = layout.serialize();
    let back = WasmLayout::deserialize(&bytes).expect("round-trip must succeed");
    assert_eq!(back.n_slots, 7);
    assert_eq!(back.n_chunks, 101);
    assert_eq!(back.results_offset, 112);
    assert_eq!(back.var_offsets, layout.var_offsets);
    // The GF offsets are not serialized; they reconstruct as 0.
    assert_eq!(back.gf_directory_offset, 0);
    assert_eq!(back.gf_data_offset, 0);
}

/// Task 3 (serializer robustness): a truncated buffer deserializes to `None`
/// rather than panicking, so a host handed a corrupt buffer fails cleanly.
#[test]
fn wasm_layout_deserialize_truncated_is_none() {
    let layout = WasmLayout {
        n_slots: 2,
        n_chunks: 3,
        results_offset: 32,
        gf_directory_offset: 0,
        gf_data_offset: 0,
        var_offsets: vec![("x".to_string(), 0), ("y".to_string(), 1)],
    };
    let bytes = layout.serialize();
    // Every strict prefix of a valid buffer must fail to parse (each cuts off
    // a length-prefixed field mid-way).
    for cut in 0..bytes.len() {
        assert!(
            WasmLayout::deserialize(&bytes[..cut]).is_none(),
            "a buffer truncated to {cut} bytes must not deserialize"
        );
    }
    // The full buffer parses.
    assert!(WasmLayout::deserialize(&bytes).is_some());
}

/// Task 1 (pure layout): an empty table list yields no regions and no
/// growth, so a model without graphical functions is unaffected.
#[test]
fn build_gf_regions_empty_is_none() {
    assert!(
        build_gf_regions(&[], 4096)
            .expect("layout must succeed")
            .is_none(),
        "no tables -> no GF regions"
    );
}

/// Task 1 (data-section round-trip): the GF regions reach the instantiated
/// module's linear memory via the active `DataSection`, at the bases the
/// directory advertises. Reads the directory entry for table 0 from memory,
/// follows its absolute data offset, and asserts the `(x, y)` knots are
/// present with the right count -- the contract the `Lookup` opcode (Task 3)
/// relies on. (Exercised end-to-end through a GF *model* once the opcode
/// lowers, in `compile_simulation_gf_lookup_modes_match_vm`.)
#[test]
fn assembled_module_initializes_gf_regions_in_memory() {
    let knots = [(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];
    let region_base = WASM_PAGE_SIZE; // one page in, comfortably past slot 0
    let regions = build_gf_regions(std::slice::from_ref(&knots.to_vec()), region_base)
        .expect("layout")
        .expect("non-empty");

    // A minimal module: one empty exported `run` (so the assembler shape is
    // exercised) is unnecessary here -- assert directly that the active data
    // segments initialize memory. Assemble via the production assembler with
    // a single root instance of three empty (0-input) program functions.
    let helpers = build_helpers();
    let empty = || {
        let mut f = Function::new([]);
        f.instruction(&I::End);
        f
    };
    let pages = (region_base + regions.total_bytes)
        .div_ceil(WASM_PAGE_SIZE)
        .max(1);
    let empty_const_init = ConstRegionInit {
        value_segments: Vec::new(),
        valid_segments: Vec::new(),
    };
    let wasm = assemble_simulation(AssembleParts {
        helpers,
        program_fns: vec![empty(), empty(), empty()],
        run_fn: empty(),
        // Empty (no-op) override functions: this test only checks the GF data
        // segments, so the override exports are present but trivial.
        set_value_fn: {
            let mut f = Function::new([]);
            // A `(i32, f64) -> i32` body must leave an i32 on the stack.
            f.instruction(&I::I32Const(0));
            f.instruction(&I::End);
            f
        },
        reset_fn: empty(),
        clear_values_fn: empty(),
        // Empty (no-op) resumable-run functions: this test only checks the GF
        // data segments. `run_to` is `(f64) -> ()` and `run_initials` is
        // `() -> ()`; an empty body type-checks against either (the type comes
        // from the function section, and a no-op leaves the stack empty).
        run_to_fn: empty(),
        run_initials_fn: empty(),
        // The real `get_error` body: it reads the two error globals, which the
        // assembler emits unconditionally, so an empty body would not type-check
        // against its `() -> i64` signature anyway.
        get_error_fn: super::errors::emit_get_error(),
        // No queue pass: no container-skipping initials, no bump-pointer global.
        initials_skipping_fn: None,
        instance_input_counts: &[0],
        pages,
        n_slots: 0,
        n_chunks: 0,
        results_base: 0,
        heap_base: None,
        gf_regions: &[&regions],
        const_init: &empty_const_init,
        belt_init_data: &[],
    });

    let info = validate(&wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();

    let dir_off = regions.directory_base as usize;
    let (data_off, n_points, flat) = store.mem_access_mut_slice(mem, |bytes| {
        let data_off = i32::from_le_bytes(bytes[dir_off..dir_off + 4].try_into().unwrap()) as usize;
        let n_points =
            i32::from_le_bytes(bytes[dir_off + 4..dir_off + 8].try_into().unwrap()) as usize;
        let flat: Vec<f64> = (0..n_points * 2)
            .map(|i| {
                let a = data_off + i * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect();
        (data_off, n_points, flat)
    });

    assert_eq!(n_points, knots.len(), "directory point count");
    assert_eq!(
        data_off, regions.data_base as usize,
        "table 0's data offset is the start of the data region"
    );
    for (k, &(x, y)) in knots.iter().enumerate() {
        assert_eq!(flat[2 * k], x, "knot {k} x");
        assert_eq!(flat[2 * k + 1], y, "knot {k} y");
    }
}

/// Task 3 (end-to-end): a model with a graphical-function variable looked up
/// in all three modes -- `LOOKUP` (Interpolate), `LOOKUP FORWARD`, and
/// `LOOKUP BACKWARD` -- matches the VM at every saved step. The lookup index
/// is `TIME - 1`, which sweeps the table's x-domain plus a below-range
/// margin (negative at t=0) and an above-range margin, so the recorded
/// series exercise below/at-knot/between/above across the run.
#[test]
fn compile_simulation_gf_lookup_modes_match_vm() {
    let knots = [(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];
    let datamodel = crate::test_common::TestProject::new("gf_modes")
        // TIME 0..6, dt 0.25 -> index = TIME-1 sweeps -1..5 over [0,4] table.
        .with_sim_time(0.0, 6.0, 0.25)
        .aux("input", "TIME - 1", None)
        .aux_with_gf("curve", "0", gf_from_knots(&knots))
        .aux("interp_val", "LOOKUP(curve, input)", None)
        .aux("fwd_val", "LOOKUP_FORWARD(curve, input)", None)
        .aux("bwd_val", "LOOKUP_BACKWARD(curve, input)", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let checked = assert_matches_vm(sim, &artifact);
    // All five variables must reach parity: the three lookup-mode results
    // (interp/fwd/bwd), the lookup-only `curve` holder they read, and its
    // `input`. Pinning >= 5 (not just the 3 lookup modes) proves the
    // lookup-only curve holder and its driver also match the VM.
    assert!(
        checked >= 5,
        "expected to compare interp/fwd/bwd + curve + input, only checked {checked}"
    );
    for name in ["interp_val", "fwd_val", "bwd_val"] {
        assert!(
            artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
            "{name} should be in the layout"
        );
    }
}

/// GH #924, the inverse of the GH #884 reject this test used to pin: a CONVEYOR
/// model lowers through the PUBLIC wasm datamodel entry points, with no up-front
/// marker scan and no silent VM fallback. Both LTM flag settings take the same
/// special-stock dispatch (which compiles an always-`ltm_enabled == false`
/// expanded project -- the documented conveyors.md §9 degradation), so an
/// `ltm_enabled` compile must succeed rather than trip a reject.
///
/// The `build_vm` companion is kept as the oracle-of-record: both backends build
/// the same fixture, which is the whole point of removing the gate. Slab-level
/// parity for this and every other belt feature lives in `wasmgen::belt`'s tests,
/// and the end-to-end corpus gate in `tests/integration/simulate.rs`.
#[test]
fn conveyor_models_lower_through_the_datamodel_entry_point() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel =
        open_xmile(&mut BufReader::new(xml.as_bytes())).expect("parse conveyor fixture");
    let main = datamodel.models[0].name.clone();

    for ltm_enabled in [false, true] {
        let artifact = compile_datamodel_to_artifact(&datamodel, &main, ltm_enabled, false)
            .unwrap_or_else(|e| {
                panic!("a conveyor model must lower (ltm_enabled={ltm_enabled}): {e:?}")
            });
        validate(&artifact.wasm).expect("the conveyor blob must validate under the interpreter");
        assert!(
            artifact
                .layout
                .var_offsets
                .iter()
                .any(|(n, _)| n == "students"),
            "the conveyor stock must be in the layout"
        );
    }

    let mut vm = crate::queue_compile::build_vm(&datamodel, &main).expect("VM must build");
    vm.run_to_end().expect("VM must run the conveyor fixture");
}

/// The queue sibling of [`conveyor_models_lower_through_the_datamodel_entry_point`].
/// Kept here (rather than only in `wasmgen::passes`'s tests) so the two special-stock
/// kinds' entry-point contract stays visible side by side.
#[test]
fn queue_models_lower_through_the_datamodel_entry_point() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = open_xmile(&mut BufReader::new(xml.as_bytes())).expect("parse queue fixture");
    let main = datamodel.models[0].name.clone();

    let artifact = compile_datamodel_to_artifact(&datamodel, &main, false, false)
        .expect("a queue model must lower to wasm");
    validate(&artifact.wasm).expect("the queue blob must validate under the interpreter");
    assert!(
        artifact
            .layout
            .var_offsets
            .iter()
            .any(|(n, _)| n == "waiting"),
        "the queue stock must be in the layout"
    );
}

/// The FFI entry point goes through the salsa pipeline + `compile_simulation`
/// and returns a non-empty blob that validates under the interpreter.
#[test]
fn compile_datamodel_to_wasm_validates() {
    let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
    let mut reader = BufReader::new(file);
    let datamodel = open_xmile(&mut reader).expect("parse population xmile");

    let wasm = compile_datamodel_to_wasm(&datamodel, "main", false, false).expect("wasm codegen");
    assert!(!wasm.is_empty(), "blob should be non-empty");
    validate(&wasm).expect("blob must validate under the interpreter");
}

// ── compile_simulation (CompiledSimulation -> wasm) ───────────────────

/// Build a `CompiledSimulation` for the named model of `datamodel` via the
/// production incremental pipeline (the same path the VM corpus uses).
fn compile_sim(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> std::sync::Arc<CompiledSimulation> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    compile_project_incremental(&db, sync.project, model_name).expect("incremental compile")
}

/// Run a `WasmArtifact` under the DLR-FT interpreter and return the
/// step-major results slab (`n_chunks * n_slots` f64, row-major by step).
fn run_artifact_results(artifact: &WasmArtifact) -> Vec<f64> {
    let info = validate(&artifact.wasm).expect("generated module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run = store
        .instance_export(inst, "run")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run wasm");
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
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

/// Assert every variable in `artifact.layout` matches the VM's series for
/// the same `CompiledSimulation`. Returns the number of variables checked.
fn assert_matches_vm(sim: std::sync::Arc<CompiledSimulation>, artifact: &WasmArtifact) -> usize {
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let wasm_data = run_artifact_results(artifact);

    let mut vm = Vm::new(sim).expect("vm creation");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();

    assert_eq!(
        vm_results.step_count, n_chunks,
        "saved-chunk count differs from VM"
    );

    let mut checked = 0usize;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_data[c * n_slots + wasm_off];
            let diff = (vm_val - wasm_val).abs();
            assert!(
                diff < 1e-9,
                "{name} mismatch at chunk {c}: vm={vm_val} wasm={wasm_val} (diff {diff})",
            );
        }
        checked += 1;
    }
    checked
}

/// End-to-end VM parity for the `AllocateAvailable` opcode on the real
/// `allocate.xmile` corpus model. The model's supply ramps from 0 to 10
/// over the run while total demand is 9, so the recorded series sweep all
/// three regimes -- `avail <= 0` (zeros) early, the partial-allocation
/// bisection over rectangular priority profiles in the middle, and
/// `avail >= total_demand` (full grant) once supply exceeds demand --
/// against `Vm::new(sim).run_to_end()`. (The model is NOT in the active
/// `wasm_parity_floor` corpus; raising that floor is a separate task.)
#[test]
fn compile_simulation_allocate_available_matches_vm() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/sdeverywhere/models/allocate/allocate.xmile"
    );
    let file = std::fs::File::open(path).expect("open allocate xmile");
    let mut reader = BufReader::new(file);
    let datamodel = open_xmile(&mut reader).expect("parse allocate xmile");
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("allocate wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 5,
        "expected to compare the allocate model's variables, only checked {checked}"
    );
    assert!(
        artifact
            .layout
            .var_offsets
            .iter()
            .any(|(n, _)| n.starts_with("shipments")),
        "the arrayed shipments allocation should be in the layout"
    );
}

#[test]
fn compile_simulation_population_matches_vm() {
    let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
    let mut reader = BufReader::new(file);
    let datamodel = open_xmile(&mut reader).expect("parse population xmile");

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    // Geometry is self-consistent with the specs.
    let specs = Specs::from(&datamodel.sim_specs);
    assert_eq!(artifact.layout.n_chunks, specs.n_chunks);

    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 5,
        "expected to compare the population model's variables, only checked {checked}"
    );
    assert!(
        artifact
            .layout
            .var_offsets
            .iter()
            .any(|(n, _)| n == "population"),
        "the population stock should be in the layout"
    );
}

#[test]
fn compile_simulation_simple_stock_flow_matches_vm() {
    // A minimal scalar Euler model: a stock filled by a constant inflow.
    let datamodel = crate::test_common::TestProject::new("simple")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare level + inflow");
    // level should integrate to 2*10 = 20 by the last step.
    let last = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;
    let level_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "level")
        .map(|(_, off)| *off)
        .expect("level offset");
    let last_step = (artifact.layout.n_chunks - 1) * n_slots + level_off;
    assert!(
        (last[last_step] - 20.0).abs() < 1e-9,
        "level should reach 20"
    );
}

#[test]
fn compile_simulation_save_step_cadence_matches_vm() {
    // Exercises the conditional-save / non-save-step copy-back branch of
    // `save_advance!` (`vm.rs:682`): with save_step = 2*dt, most steps copy
    // `next -> curr` WITHOUT recording a snapshot, and only every other step
    // (plus the forced t=start sample) writes a results row. Every other
    // wasmgen test uses save_step = None (save_every = 1), so this is the
    // only coverage of the multi-step cadence.
    let mut datamodel = crate::test_common::TestProject::new("cadence")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();
    // `with_sim_time` clears save_step to dt; the builder has no
    // `with_save_step`, so set it directly: save_step = 2, dt = 1.
    datamodel.sim_specs.save_step = Some(crate::datamodel::Dt::Dt(2.0));

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    // dt=1, save_step=2 over [0,10] saves at t=0,2,4,6,8,10 -> 6 chunks.
    assert_eq!(
        artifact.layout.n_chunks, 6,
        "save_step = 2*dt over [0,10] should yield 6 saved samples"
    );

    // Per-variable series + saved-chunk count both match the VM (which
    // `assert_matches_vm` asserts via `step_count == n_chunks`).
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare level + inflow");
}

#[test]
fn compile_simulation_conditional_model_matches_vm() {
    // Exercises the SetCond/If lowering through the whole-model path.
    let datamodel = crate::test_common::TestProject::new("cond")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("threshold", "3", None)
        .aux("gated", "IF TIME > threshold THEN 10 ELSE 1", None)
        .stock("acc", "0", &["gated_flow"], &[], None)
        .flow("gated_flow", "gated", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare gated + acc");
}

// ── PREVIOUS / INIT (Task 1: snapshot regions + LoadPrev/LoadInitial) ──

/// Task 1: `PREVIOUS(x)` under Euler. At t0 the snapshot has not been taken,
/// so `LoadPrev` returns its fallback (the 0 the unary `PREVIOUS` desugars
/// to); after the first step it returns the prior step's `x`. The series
/// must match the VM, which gates the same fallback-vs-snapshot choice on
/// `use_prev_fallback`.
#[test]
fn compile_simulation_previous_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("prev")
        .with_sim_time(0.0, 5.0, 1.0)
        // x ramps each step so PREVIOUS(x) is a visibly-lagged series.
        .stock("x", "10", &["grow"], &[], None)
        .flow("grow", "1", None)
        .aux("x_prev", "PREVIOUS(x)", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare x + x_prev");
}

/// Instantiate `artifact` ONCE and invoke the exported `run` `runs` times in
/// sequence with no `reset` between, returning the results slab read after
/// each call. Models the wasm backend's documented "instantiate once, re-run
/// on every change" usage (interactive scrubbing; the POC's `run` "re-runs
/// the whole simulation" per call) -- which exercises the cross-run state
/// reset that a single `run` invocation cannot.
fn run_artifact_results_repeated(artifact: &WasmArtifact, runs: usize) -> Vec<Vec<f64>> {
    let info = validate(&artifact.wasm).expect("generated module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let n = artifact.layout.n_chunks * artifact.layout.n_slots;
    let base = artifact.layout.results_offset;
    let mut out = Vec::with_capacity(runs);
    for _ in 0..runs {
        let run = store
            .instance_export(inst, "run")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(), ()>(run, ())
            .expect("run wasm");
        let mem = store
            .instance_export(inst, "memory")
            .unwrap()
            .as_mem()
            .unwrap();
        let slab = store.mem_access_mut_slice(mem, |bytes| {
            (0..n)
                .map(|i| {
                    let a = base + i * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect::<Vec<f64>>()
        });
        out.push(slab);
    }
    out
}

/// Regression (PR #620 review): `run` reseeds the time globals and reruns
/// initials, so it is a complete simulation from t0 and the documented
/// per-change entry point for repeated re-simulation. It must therefore
/// reset the PREVIOUS fallback flag itself, mirroring the VM's `run_initials`
/// (which sets `use_prev_fallback = true` at the start of every run). Without
/// that reset, the loop leaves the flag at 0, so a SECOND `run` on the same
/// instance reads the first run's final `prev_values` on step 0 (and during
/// initials) instead of the fallback -- contaminating any `PREVIOUS(...)`
/// model. This instantiates once and runs twice with no `reset` between: a
/// deterministic model must produce identical results both times, and
/// `x_prev` at t0 must be the unary-PREVIOUS fallback (0), not the stale
/// prior-run value.
#[test]
fn compile_simulation_repeated_run_resets_previous_fallback() {
    let datamodel = crate::test_common::TestProject::new("prev_repeat")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("x", "10", &["grow"], &[], None)
        .flow("grow", "1", None)
        .aux("x_prev", "PREVIOUS(x)", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let runs = run_artifact_results_repeated(&artifact, 2);
    let (first, second) = (&runs[0], &runs[1]);

    // A deterministic model re-run from t0 produces byte-identical results;
    // the bug makes the second run's PREVIOUS reads diverge on step 0.
    assert_eq!(
        first, second,
        "second run() diverged from the first -- stale PREVIOUS fallback state leaked across runs"
    );

    // Pin the discriminating cell: x_prev at the first saved chunk (t0) is
    // the unary-PREVIOUS fallback (0), not the prior run's final x.
    let x_prev_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(name, _)| name == "x_prev")
        .map(|(_, off)| *off)
        .expect("x_prev in layout");
    assert_eq!(
        second[x_prev_off], 0.0,
        "x_prev at t0 on the second run must be the PREVIOUS fallback (0), got {}",
        second[x_prev_off]
    );
}

/// The ARRAY twin of the test above, and the one thing the corpus gate cannot
/// reach (GH #995): an array-valued `PREVIOUS` is a VIEW over `prev_values`, and
/// a view read is a plain `f64.load` at a constant address -- nothing about it
/// consults `use_prev_fallback` unless the emitter puts a `select` there.
///
/// On a FIRST run that omission is invisible: wasm linear memory starts zeroed,
/// so the snapshot region reads 0 anyway, which is exactly the fallback. It only
/// shows up on a second run, because the blob's `reset` deliberately does NOT
/// clear the snapshot regions (it sets the flag instead, which is all the scalar
/// `LoadPrev` needs). So the second run's step 0 would read the FIRST run's
/// final `prev_values` -- a plausible array of stale numbers, no diagnostic.
///
/// `SUM(PREVIOUS(x[*]))` is 0 at t=0 and the previous step's total afterwards;
/// the stale reading is the first run's last total (42), which is what this pins
/// against.
#[test]
fn compile_simulation_repeated_run_resets_previous_fallback_for_an_array_view() {
    let datamodel = crate::test_common::TestProject::new("prev_array_repeat")
        .with_sim_time(0.0, 5.0, 1.0)
        .indexed_dimension("d", 3)
        .array_stock("x[d]", "10", &["grow"], &[], None)
        .array_flow("grow[d]", "1", None)
        .aux("x_prev_sum", "SUM(PREVIOUS(x[*]))", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let runs = run_artifact_results_repeated(&artifact, 2);
    let (first, second) = (&runs[0], &runs[1]);
    assert_eq!(
        first, second,
        "second run() diverged from the first -- a PREVIOUS VIEW read the stale \
         snapshot region instead of the fallback"
    );

    let off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(name, _)| name == "x_prev_sum")
        .map(|(_, off)| *off)
        .expect("x_prev_sum in layout");
    assert_eq!(
        second[off], 0.0,
        "SUM(PREVIOUS(x[*])) at t0 on the second run must be the fallback 0, not \
         the first run's final total (42); got {}",
        second[off]
    );
    // ... and the step after t0 must be the real previous total (3 * 10), so the
    // fallback is not being returned forever.
    let n_slots = artifact.layout.n_slots;
    assert_eq!(
        second[n_slots + off],
        30.0,
        "the step after the fallback must read the real snapshot"
    );
}

/// Regression (PR #620 review): a stock at an absolute slot offset >= 65536
/// must address its real slot under RK integration, not `off & 0xFFFF`. Such
/// offsets are reachable in a large nested model (each submodel/SMOOTH/DELAY
/// instance adds slots; nothing caps total `n_slots` in the wasm path). The
/// RK stage delta `next[off] - curr[off]` is computed by
/// `emit_compute_stage_delta`; the original bug threaded `off` as `u16`, so a
/// stock at offset 65536 read slot `65536 & 0xFFFF == 0` (TIME) instead of its
/// own. This drives the helper at offset 65536 over a hand-built memory whose
/// slot 0 and slot 65536 hold distinct values and asserts it reads slot 65536
/// (matching the Euler advance, which has always used the full-width offset).
#[test]
fn rk_stage_delta_addresses_stock_above_65535() {
    // 65536 & 0xFFFF == 0, so a truncated offset would alias slot 0 (TIME).
    const HIGH_OFF: u32 = 65536;
    // `curr` holds slots [0, HIGH_OFF]; `next` sits one stride past it.
    let next_base = (HIGH_OFF + 1) * SLOT_SIZE;

    // probe() -> f64: L_RK_S := next[HIGH_OFF] - curr[HIGH_OFF]; return it.
    // Locals mirror the run fn so the f64 local L_RK_S (index 4) is valid.
    let mut probe = Function::new([(3, ValType::I32), (2, ValType::F64)]);
    emit_compute_stage_delta(&mut probe, next_base, HIGH_OFF);
    probe.instruction(&I::LocalGet(L_RK_S));
    probe.instruction(&I::End);

    let mut module = WasmModule::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::F64]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let bytes_needed = next_base + (HIGH_OFF + 1) * SLOT_SIZE;
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: u64::from(bytes_needed.div_ceil(65536) + 1),
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export("probe", ExportKind::Func, 0);
    exports.export("memory", ExportKind::Memory, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    code.function(&probe);
    module.section(&code);
    let wasm = module.finish();

    let info = validate(&wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    // Seed slot 0 (the alias target under truncation) and slot HIGH_OFF with
    // distinct values, so reading the wrong slot yields a distinguishable result.
    let curr_hi = (HIGH_OFF * SLOT_SIZE) as usize;
    let next0 = next_base as usize;
    let next_hi = (next_base + HIGH_OFF * SLOT_SIZE) as usize;
    store.mem_access_mut_slice(mem, |b| {
        b[0..8].copy_from_slice(&100.0f64.to_le_bytes()); // curr[0]
        b[next0..next0 + 8].copy_from_slice(&200.0f64.to_le_bytes()); // next[0]
        b[curr_hi..curr_hi + 8].copy_from_slice(&3.0f64.to_le_bytes()); // curr[HIGH_OFF]
        b[next_hi..next_hi + 8].copy_from_slice(&10.0f64.to_le_bytes()); // next[HIGH_OFF]
    });
    let probe_fn = store
        .instance_export(inst, "probe")
        .unwrap()
        .as_func()
        .unwrap();
    let delta: f64 = store
        .invoke_simple_typed::<(), f64>(probe_fn, ())
        .expect("probe");

    // next[HIGH_OFF] - curr[HIGH_OFF] = 10 - 3 = 7. A truncated u16 offset
    // would read slot 0 instead (200 - 100 = 100).
    assert_eq!(
        delta, 7.0,
        "RK stage delta read the wrong slot -- stock offset truncated above 65535?"
    );
}

/// Task 1: `INIT(x)` referenced from a flow reads the `initial_values`
/// snapshot captured once after the initials phase (in the flows/stocks
/// programs `LoadInitial` reads `initial_values[off]`, never `curr`). Here
/// the inflow is held at `INIT(level)`, so `level` integrates by its own
/// initial value each step; the wasm series must match the VM.
#[test]
fn compile_simulation_init_from_flow_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("init_flow")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("level", "7", &["inflow"], &[], None)
        // INIT(level) is captured once at t0 (= 7) and stays 7 every step.
        .flow("inflow", "INIT(level)", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare level + inflow");
    // level starts at 7 and grows by INIT(level)=7 each of 5 steps -> 42.
    let results = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;
    let level_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "level")
        .map(|(_, off)| *off)
        .expect("level offset");
    let last = (artifact.layout.n_chunks - 1) * n_slots + level_off;
    assert!(
        (results[last] - 42.0).abs() < 1e-9,
        "level should reach 7 + 5*7 = 42, got {}",
        results[last]
    );
}

/// Task 1: `INIT(x)` referenced from *another initial equation* reads
/// `curr` during the initials phase (the snapshot is taken only after
/// initials run). `seed` is computed during initials, and `derived`'s
/// initial equation reads `INIT(seed)` -- which must resolve to the
/// just-computed `curr[seed]`, not an as-yet-unwritten `initial_values`.
#[test]
fn compile_simulation_init_from_initial_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("init_initial")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("seed", "5", None)
        // A stock whose INITIAL equation reads INIT(seed): during initials
        // LoadInitial must read curr[seed] (= 5), so derived starts at 5.
        .stock("derived", "INIT(seed)", &["hold"], &[], None)
        .flow("hold", "0", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare seed + derived");
    // derived initializes to INIT(seed)=5 and the flow holds it there.
    // Chunk 0 starts at slab offset 0, so `derived_off` indexes it directly.
    let results = run_artifact_results(&artifact);
    let derived_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "derived")
        .map(|(_, off)| *off)
        .expect("derived offset");
    assert!(
        (results[derived_off] - 5.0).abs() < 1e-9,
        "derived should initialize to INIT(seed) = 5, got {}",
        results[derived_off]
    );
}

/// An INIT-only capture is populated by initials and never refreshed in
/// flows, on both backends: the frozen user value matches the VM step for
/// step, a second `run` on the same instance reseeds it, and the capture's
/// slot -- which the VM's fresh step chunks leave at zero where wasm's linear
/// memory keeps the initial value -- is in neither backend's results map, so
/// the map exposes only slots the two agree on.
#[test]
fn compile_simulation_init_capture_without_flow_refresh_matches_vm_and_reruns() {
    let datamodel = crate::test_common::TestProject::new("init_capture_wasm")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("driver", "1 + TIME", None)
        .aux("frozen", "INIT(driver * 2)", None)
        .build_datamodel();

    let sim = compile_sim(&datamodel, "main");
    let capture = "$\u{205A}frozen\u{205A}0\u{205A}arg0";
    assert!(
        sim.offsets.keys().all(|name| name.as_str() != capture),
        "an INIT-only capture has no results key"
    );
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    assert!(
        artifact
            .layout
            .var_offsets
            .iter()
            .all(|(name, _)| name != capture),
        "the wasm layout is the same map"
    );
    let runs = run_artifact_results_repeated(&artifact, 2);
    assert_eq!(
        runs[0], runs[1],
        "a second run on one instance reseeds INIT"
    );

    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare driver + frozen");
    let frozen_offset = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(name, _)| name == "frozen")
        .map(|(_, offset)| *offset)
        .expect("frozen offset");
    assert!(runs.iter().all(|run| {
        (0..artifact.layout.n_chunks)
            .all(|step| run[step * artifact.layout.n_slots + frozen_offset] == 2.0)
    }));
}

// ── RK2 / RK4 integration loops (Task 2) ──────────────────────────────

/// A logistic-growth model: `pop' = rate * pop * (1 - pop/capacity)`. The
/// nonlinear flow depends on the stock, so RK's trial-point evaluations
/// genuinely differ from Euler -- a pure-constant flow would let a broken RK
/// loop pass by coincidence.
fn logistic_growth(name: &str, method: crate::datamodel::SimMethod) -> crate::datamodel::Project {
    crate::test_common::TestProject::new(name)
        .with_sim_time(0.0, 20.0, 0.5)
        .with_sim_method(method)
        .aux("rate", "0.3", None)
        .aux("capacity", "1000", None)
        .stock("pop", "10", &["growth"], &[], None)
        .flow("growth", "rate * pop * (1 - pop / capacity)", None)
        .build_datamodel()
}

/// Task 2: an RK4 scalar model matches the VM's saved samples (cadence and
/// values). The VM's RK4 loop is the oracle; the emitted four-stage loop
/// with time juggling + the end-of-step flows re-eval must reproduce it.
#[test]
fn compile_simulation_rk4_matches_vm() {
    let datamodel = logistic_growth("rk4_logistic", crate::datamodel::SimMethod::RungeKutta4);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (RK4)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare pop + growth");
}

/// Task 2: an RK2 (Heun) scalar model matches the VM's saved samples. Same
/// nonlinear model so the two-stage trial step is genuinely exercised.
#[test]
fn compile_simulation_rk2_matches_vm() {
    let datamodel = logistic_growth("rk2_logistic", crate::datamodel::SimMethod::RungeKutta2);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (RK2)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 2, "expected to compare pop + growth");
}

/// Task 2: RK4 and RK2 must genuinely differ from Euler on this nonlinear
/// model -- otherwise the RK tests above could pass against a loop that
/// silently fell back to Euler. Establishes that the oracle (the VM) sees a
/// method-dependent trajectory, so wasm-vs-VM parity is a meaningful check.
#[test]
fn rk_methods_differ_from_euler_in_vm() {
    let last_pop = |method| {
        let datamodel = logistic_growth("rk_vs_euler", method);
        let sim = compile_sim(&datamodel, "main");
        let mut vm = Vm::new(sim).expect("vm");
        vm.run_to_end().expect("vm run");
        let results = vm.into_results();
        let pop = Ident::<Canonical>::from_str_unchecked("pop");
        let off = *results.offsets.get(&pop).expect("pop offset");
        results.data[(results.step_count - 1) * results.step_size + off]
    };
    let euler = last_pop(crate::datamodel::SimMethod::Euler);
    let rk4 = last_pop(crate::datamodel::SimMethod::RungeKutta4);
    let rk2 = last_pop(crate::datamodel::SimMethod::RungeKutta2);
    assert!(
        (euler - rk4).abs() > 1e-6,
        "RK4 must differ from Euler (euler={euler}, rk4={rk4})"
    );
    assert!(
        (euler - rk2).abs() > 1e-6,
        "RK2 must differ from Euler (euler={euler}, rk2={rk2})"
    );
}

/// A coupled two-stock Lotka-Volterra (predator-prey) model. Each stock's
/// flows read the *other* stock, so a single RK stage's trial-point
/// evaluation interleaves both stocks: `prey`'s `predation` outflow reads
/// `predator`, and `predator`'s `growth` inflow reads `prey`. This is what
/// the single-stock RK tests cannot exercise -- with two stocks the stage
/// math walks `stock_offsets` and keeps each stock's `saved[i]`/`accum[i]`
/// and trial `curr[off_i]` independent. A loop that aliased the scratch
/// across stocks, or iterated `stock_offsets` in an unstable order, would
/// corrupt one stock's trajectory and fail the VM-parity check below.
///
/// Classic textbook parameters (alpha/beta/gamma/delta) on a short horizon
/// with a small dt: the system oscillates, both stay strictly positive, and
/// Euler vs RK4/RK2 visibly diverge (asserted by
/// `multi_stock_coupled_diverges_euler_vs_rk_in_vm`). 100 steps keeps the
/// un-JITed DLR-FT run well under the per-test budget.
fn lotka_volterra(name: &str, method: crate::datamodel::SimMethod) -> crate::datamodel::Project {
    crate::test_common::TestProject::new(name)
        .with_sim_time(0.0, 5.0, 0.05)
        .with_sim_method(method)
        .aux("alpha", "1.1", None)
        .aux("beta", "0.4", None)
        .aux("gamma", "0.4", None)
        .aux("delta", "0.1", None)
        // prey:     d/dt = alpha*prey - beta*prey*predator
        .stock("prey", "10", &["prey_birth"], &["predation"], None)
        .flow("prey_birth", "alpha * prey", None)
        .flow("predation", "beta * prey * predator", None)
        // predator: d/dt = delta*prey*predator - gamma*predator
        .stock("predator", "10", &["pred_growth"], &["pred_death"], None)
        .flow("pred_growth", "delta * prey * predator", None)
        .flow("pred_death", "gamma * predator", None)
        .build_datamodel()
}

/// Meaningfulness precondition for the two-stock RK parity tests: the
/// coupled model's trajectory is genuinely method-dependent in the VM (the
/// oracle) for *both* stocks. Without this, a wasm RK loop that silently
/// degraded to Euler -- or never advanced the second stock -- could pass
/// `assert_matches_vm` against a coincidentally-identical VM Euler series.
#[test]
fn multi_stock_coupled_diverges_euler_vs_rk_in_vm() {
    let last_two = |method| {
        let datamodel = lotka_volterra("lv_vs_euler", method);
        let sim = compile_sim(&datamodel, "main");
        let mut vm = Vm::new(sim).expect("vm");
        vm.run_to_end().expect("vm run");
        let results = vm.into_results();
        let read = |name: &str| {
            let id = Ident::<Canonical>::from_str_unchecked(name);
            let off = *results
                .offsets
                .get(&id)
                .unwrap_or_else(|| panic!("{name} offset"));
            results.data[(results.step_count - 1) * results.step_size + off]
        };
        (read("prey"), read("predator"))
    };
    let (e_prey, e_pred) = last_two(crate::datamodel::SimMethod::Euler);
    let (rk4_prey, rk4_pred) = last_two(crate::datamodel::SimMethod::RungeKutta4);
    let (rk2_prey, rk2_pred) = last_two(crate::datamodel::SimMethod::RungeKutta2);
    // Both stocks must move under RK4 and RK2 relative to Euler -- proving
    // the stage math integrates each independently, not just the first.
    assert!(
        (e_prey - rk4_prey).abs() > 1e-6 && (e_pred - rk4_pred).abs() > 1e-6,
        "RK4 must differ from Euler for both stocks \
         (prey: euler={e_prey} rk4={rk4_prey}; predator: euler={e_pred} rk4={rk4_pred})"
    );
    assert!(
        (e_prey - rk2_prey).abs() > 1e-6 && (e_pred - rk2_pred).abs() > 1e-6,
        "RK2 must differ from Euler for both stocks \
         (prey: euler={e_prey} rk2={rk2_prey}; predator: euler={e_pred} rk2={rk2_pred})"
    );
}

/// Coverage gap closed: a TWO-STOCK COUPLED model under RK4 matches the VM
/// per-variable, per-chunk. The phase's other RK tests are single-stock, so
/// this is the only check that the four-stage stage math keeps two stocks'
/// `saved[i]`/`accum[i]`/`curr[off_i]` independent and iterates
/// `stock_offsets` in a stable order across all four stages. `checked >= 2`
/// pins that both stocks (not just `prey`) reached parity.
#[test]
fn compile_simulation_two_stock_coupled_rk4_matches_vm() {
    let datamodel = lotka_volterra("lv_rk4", crate::datamodel::SimMethod::RungeKutta4);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (two-stock RK4)");
    let checked = assert_matches_vm(sim, &artifact);
    // Both stocks plus the four flows and four params all match; pin >= 2 so
    // the two coupled stocks specifically are among the compared variables.
    assert!(
        checked >= 2,
        "expected to compare both prey + predator, only checked {checked}"
    );
    for name in ["prey", "predator"] {
        assert!(
            artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
            "{name} should be in the layout"
        );
    }
}

/// The RK2 (Heun) companion to `compile_simulation_two_stock_coupled_rk4_matches_vm`:
/// the two-stage trial step over two coupled stocks matches the VM.
#[test]
fn compile_simulation_two_stock_coupled_rk2_matches_vm() {
    let datamodel = lotka_volterra("lv_rk2", crate::datamodel::SimMethod::RungeKutta2);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (two-stock RK2)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare both prey + predator, only checked {checked}"
    );
}

/// Task 2: a model using `PREVIOUS`/`INIT` under RK4 matches the VM. The
/// snapshot timing is the subtle part: `prev_values` is captured AFTER the
/// end-of-step flows re-eval (with `curr` restored to time-`t` state), not
/// from a trial point. `x_prev` lags `pop`; `pop_init` reads INIT(pop).
#[test]
fn compile_simulation_rk4_with_previous_and_init_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("rk4_prev_init")
        .with_sim_time(0.0, 10.0, 0.5)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .aux("rate", "0.3", None)
        .aux("capacity", "1000", None)
        .stock("pop", "10", &["growth"], &[], None)
        .flow("growth", "rate * pop * (1 - pop / capacity)", None)
        // PREVIOUS(pop): lagged by one saved step; captured after re-eval.
        .aux("pop_prev", "PREVIOUS(pop)", None)
        // INIT(pop): the t0 snapshot (= 10), read from initial_values.
        .aux("pop_init", "INIT(pop)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (RK4 + PREVIOUS/INIT)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 4,
        "expected to compare pop + growth + pop_prev + pop_init"
    );
}

// ── Modules: EvalModule / LoadModuleInput (Phase 7 Task 1) ────────────
//
// Each unique `(model, input_set)` instance becomes its own initials/flows/
// stocks wasm function taking `(module_off: i32, in_0..in_{k-1}: f64)`. An
// `EvalModule` resolves the child instance and `call`s its function for the
// current `StepPart`, passing `module_off + decl.off` and the popped inputs;
// `LoadModuleInput` reads an input parameter. These tests assert wasm matches
// the VM for submodel-bearing models, including the SMOOTH stdlib macro (which
// expands to implicit module stocks) and the same instance at two offsets.

/// A two-model datamodel: a `main` model that instantiates `submodel`
/// `n_instances` times, wiring `in_value` (an aux in `main`) into each
/// instance's `in` input. The submodel computes `out = body` (referencing its
/// own `in`); `body_is_stock` makes `out` a stock integrating `body`, so the
/// submodel carries internal stocks reached only through `EvalModule` (the
/// nested-stock-offset case). `TestProject` only emits a single `main` model,
/// so this is built as an explicit datamodel.
fn submodel_project(
    name: &str,
    method: crate::datamodel::SimMethod,
    in_value: &str,
    body: &str,
    body_is_stock: bool,
    n_instances: usize,
) -> crate::datamodel::Project {
    use crate::datamodel;
    let mut main_vars: Vec<datamodel::Variable> = vec![datamodel::Variable::Aux(datamodel::Aux {
        ident: "in_value".to_string(),
        equation: datamodel::Equation::Scalar(in_value.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })];
    for i in 0..n_instances {
        let ident = format!("sub{i}");
        main_vars.push(datamodel::Variable::Module(datamodel::Module {
            // A module reference's `dst` is qualified with the instance name
            // (`subN.in`), not the bare input variable; an unqualified `dst`
            // silently fails to wire the input (the submodel's `in` keeps its
            // default), which would make `LoadModuleInput` untested.
            references: vec![datamodel::ModuleReference {
                src: "in_value".to_string(),
                dst: format!("{ident}.in"),
            }],
            ident,
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        }));
    }

    let out_var = if body_is_stock {
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "out".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec!["grow".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    } else {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "out".to_string(),
            equation: datamodel::Equation::Scalar(body.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    let mut submodel_vars = vec![
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "in".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: true,
                ..datamodel::Compat::default()
            },
        }),
        out_var,
    ];
    if body_is_stock {
        submodel_vars.push(datamodel::Variable::Flow(datamodel::Flow {
            ident: "grow".to_string(),
            equation: datamodel::Equation::Scalar(body.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    }

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: method,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: main_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: submodel_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: Default::default(),
        ai_information: None,
    }
}

/// A two-model datamodel like [`submodel_project`], but the submodel carries
/// its OWN overridable constant `k` (a flows-phase `AssignConstCurr`) and
/// `out = in + k`. Instantiating it `n_instances` times in `main` gives each
/// instance a DISTINCT absolute offset for its own `k` (the recursive
/// `base_off + module_decl.off` addressing), so a per-instance `set_value`
/// override on one instance's `k` must not perturb the other. `in_value` is a
/// constant wired into every instance's `in`, so the only differentiator
/// between two instances' `out` is each instance's `k` override.
fn submodel_with_constant_project(
    name: &str,
    in_value: &str,
    k_default: &str,
    n_instances: usize,
) -> crate::datamodel::Project {
    use crate::datamodel;
    let mut main_vars: Vec<datamodel::Variable> = vec![datamodel::Variable::Aux(datamodel::Aux {
        ident: "in_value".to_string(),
        equation: datamodel::Equation::Scalar(in_value.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })];
    for i in 0..n_instances {
        let ident = format!("sub{i}");
        main_vars.push(datamodel::Variable::Module(datamodel::Module {
            references: vec![datamodel::ModuleReference {
                src: "in_value".to_string(),
                dst: format!("{ident}.in"),
            }],
            ident,
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        }));
    }

    let submodel_vars = vec![
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "in".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: true,
                ..datamodel::Compat::default()
            },
        }),
        // `k` is a bare constant, so it lowers to a flows-phase
        // `AssignConstCurr` -- i.e. an overridable constant, distinct per
        // instance.
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "k".to_string(),
            equation: datamodel::Equation::Scalar(k_default.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "out".to_string(),
            equation: datamodel::Equation::Scalar("in + k".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
    ];

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 3.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: main_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: submodel_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: Default::default(),
        ai_information: None,
    }
}

/// Task 1: a model instantiating a submodel runs through wasm and matches the
/// VM. The submodel's `out` depends on its `in` input (passed from `main`), so
/// this exercises both `EvalModule` (the child `call`) and `LoadModuleInput`
/// (the child reading its passed input). Previously this construct was rejected
/// as `submodules are not supported`.
#[test]
fn compile_simulation_submodel_matches_vm() {
    let datamodel = submodel_project(
        "submod",
        crate::datamodel::SimMethod::Euler,
        "TIME + 1",
        "in * 2",
        false,
        1,
    );
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (submodel)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare main's in_value + the submodel's out, only checked {checked}"
    );
    // The submodel's output slot is in the single shared slab, addressed at
    // `module_off + off`; its layout entry confirms it was emitted.
    assert!(
        artifact
            .layout
            .var_offsets
            .iter()
            .any(|(n, _)| n.ends_with("out")),
        "the submodel's `out` should be in the layout"
    );
}

/// Task 1: `LoadModuleInput` reads the right input. The submodel's output is
/// exactly its input, and `in_value` varies with TIME, so a wrong input-param
/// index (or a missing pass-through) would diverge from the VM immediately.
#[test]
fn compile_simulation_submodel_loadmoduleinput_reads_right_input() {
    let datamodel = submodel_project(
        "passthru",
        crate::datamodel::SimMethod::Euler,
        "TIME * 3 + 1",
        "in", // out == in: a pure pass-through of the module input
        false,
        1,
    );
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (passthrough)");

    // out must equal in_value (= TIME*3+1) at every saved step.
    let results = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;
    let find = |needle: &str| {
        artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n.ends_with(needle))
            .map(|(_, o)| *o)
            .unwrap_or_else(|| panic!("{needle} offset"))
    };
    let in_off = find("in_value");
    let out_off = find("out");
    for c in 0..artifact.layout.n_chunks {
        let in_v = results[c * n_slots + in_off];
        let out_v = results[c * n_slots + out_off];
        assert!(
            (in_v - out_v).abs() < 1e-9,
            "submodel out must equal its passed input at chunk {c}: in={in_v} out={out_v}"
        );
    }
    // And the whole model matches the VM.
    assert_matches_vm(sim, &artifact);
}

/// Task 1 (the `module_off` proof): the SAME `(model, input_set)` instance,
/// instantiated twice in `main`, runs through wasm and matches the VM. Both
/// instances share one `CompiledModule` (one function triple) but run at two
/// different base offsets, so `module_off` must thread correctly into the
/// child's slab reads/writes. Each `EvalModule` passes a distinct
/// `module_off + decl.off`.
#[test]
fn compile_simulation_two_instances_same_module_matches_vm() {
    let datamodel = submodel_project(
        "twice",
        crate::datamodel::SimMethod::Euler,
        "TIME + 2",
        "in * 10",
        false,
        2,
    );
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (two instances)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 3,
        "expected to compare in_value + both instances' out, only checked {checked}"
    );
    // Both instances' outputs occupy distinct slots in the shared slab.
    let out_slots: Vec<usize> = artifact
        .layout
        .var_offsets
        .iter()
        .filter(|(n, _)| n.ends_with("out"))
        .map(|(_, o)| *o)
        .collect();
    assert_eq!(
        out_slots.len(),
        2,
        "two instances should contribute two distinct `out` slots, got {out_slots:?}"
    );
    assert_ne!(
        out_slots[0], out_slots[1],
        "the two instances must run at different module offsets"
    );
}

/// Task 1 (per-instance DISTINCT overrides -- the direct test of the
/// absolute-slot const-region addressing): the SAME `CompiledModule`,
/// instantiated twice in `main`, carries DISTINCT `set_value` overrides for
/// its own constant `k`. Each instance's `k` lives at a distinct absolute
/// offset (`base_off + module_decl.off`, the recursion in
/// `collect_overridable_defaults`); the wasm override region is indexed by
/// that absolute offset, so overriding instance 0's `k` to 100 and instance
/// 1's `k` to 200 makes each instance's `out = in + k` reflect ITS OWN
/// override. A bug that applied one override to both instances, or that
/// ignored `module_off` (writing both overrides to the same slot), would make
/// the two `out` series equal -- which the non-vacuity `assert_ne!` rejects.
///
/// This is a wasm-only correctness property: the VM is NOT a valid cell-for-
/// cell oracle for *distinct* overrides of a SHARED module, because its
/// `set_value_by_offset` mutates the module's shared bytecode literal (one
/// `literal_id` for both instances, resolved through the single shared
/// `ModuleKey`), so the second override clobbers the first and both instances
/// read the last value. The wasm backend is strictly more correct here. The
/// VM divergence is tracked separately; this test still anchors against the
/// VM in the regime where they DO agree -- both instances overridden to the
/// SAME value (`compile_simulation_two_instances_same_value_override_matches_vm`).
#[test]
fn compile_simulation_two_instances_distinct_overrides() {
    // `in_value` is the constant 7 wired into both instances' `in`, so the
    // ONLY differentiator between the two instances' `out` is each instance's
    // `k` override (default 1).
    let datamodel = submodel_with_constant_project("distinct", "7", "1", 2);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (distinct overrides)");

    let (k0_off, k1_off) = instance_k_offsets(&artifact);
    assert_ne!(
        k0_off, k1_off,
        "the two instances' `k` must occupy distinct absolute offsets"
    );
    assert!(
        sim.is_constant_offset(k0_off) && sim.is_constant_offset(k1_off),
        "each instance's `k` must be a VM-overridable constant (sub0·k={k0_off}, sub1·k={k1_off})"
    );

    // Apply DIFFERENT overrides to the two instances, then reset + run.
    let wasm_slab = run_artifact_with_overrides(&artifact, &[(k0_off, 100.0), (k1_off, 200.0)]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    // Non-vacuity: each instance's `out` reflects ITS OWN override, and the
    // two genuinely DIFFER. `in_value` is 7, so sub0·out = 7 + 100 = 107 and
    // sub1·out = 7 + 200 = 207 at every saved step. If a bug applied one
    // override to both instances (or ignored `module_off` and wrote both to
    // one slot), the two `out` series would be equal and this would fail.
    let out0_off = layout_offset(&artifact, qualified_ident("sub0", "out").as_str());
    let out1_off = layout_offset(&artifact, qualified_ident("sub1", "out").as_str());
    for c in 0..n_chunks {
        let out0 = wasm_slab[c * n_slots + out0_off];
        let out1 = wasm_slab[c * n_slots + out1_off];
        assert!(
            (out0 - 107.0).abs() < 1e-9,
            "sub0·out should be in_value(7)+k0(100)=107 at chunk {c}, got {out0}"
        );
        assert!(
            (out1 - 207.0).abs() < 1e-9,
            "sub1·out should be in_value(7)+k1(200)=207 at chunk {c}, got {out1}"
        );
        assert_ne!(
            out0, out1,
            "the two instances' outputs must DIFFER under distinct per-instance overrides"
        );
    }
}

/// Task 1 (VM parity anchor for the shared-module override path): overriding
/// BOTH instances' `k` to the SAME value matches the VM cell-for-cell. This is
/// the regime where the VM and wasm agree -- the VM's shared-literal clobber
/// (see `compile_simulation_two_instances_distinct_overrides`) is harmless
/// when both overrides carry the same value -- so it proves the wasm override
/// mechanism is faithful to the VM (not merely internally consistent) for a
/// shared `CompiledModule` instantiated at two `module_off`s.
#[test]
fn compile_simulation_two_instances_same_value_override_matches_vm() {
    let datamodel = submodel_with_constant_project("same_val", "7", "1", 2);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let (k0_off, k1_off) = instance_k_offsets(&artifact);
    let wasm_slab = run_artifact_with_overrides(&artifact, &[(k0_off, 300.0), (k1_off, 300.0)]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm creation");
    vm.set_value_by_offset(k0_off, 300.0)
        .expect("sub0·k must be a VM-overridable constant");
    vm.set_value_by_offset(k1_off, 300.0)
        .expect("sub1·k must be a VM-overridable constant");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    assert_eq!(
        vm_results.step_count, n_chunks,
        "saved-chunk count differs from VM"
    );

    let mut checked = 0usize;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} mismatch at chunk {c} under same-value override: \
                 vm={vm_val} wasm={wasm_val}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected to compare in_value + both instances' k/out, only checked {checked}"
    );
    // Both instances reach 7 + 300 = 307 (the override took on both).
    let out0_off = layout_offset(&artifact, qualified_ident("sub0", "out").as_str());
    let out1_off = layout_offset(&artifact, qualified_ident("sub1", "out").as_str());
    assert!(
        (wasm_slab[out0_off] - 307.0).abs() < 1e-9 && (wasm_slab[out1_off] - 307.0).abs() < 1e-9,
        "both instances should reach 7+300=307 under the shared override"
    );
}

/// Task 1 (nested stocks under Euler): a submodel whose `out` is a stock
/// integrating a flow that depends on its `in` input. The submodel's internal
/// stock is reached only through `EvalModule`, and its offset must be picked
/// up by the recursive stock-offset collection so the Euler advance copies it
/// `next -> curr`. The wasm must match the VM.
#[test]
fn compile_simulation_submodel_nested_stock_euler_matches_vm() {
    let datamodel = submodel_project(
        "nested_stock",
        crate::datamodel::SimMethod::Euler,
        "2",
        "in", // grow = in (= 2); out integrates by 2 each step
        true,
        1,
    );
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (nested stock)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare in_value + nested out stock"
    );
    // Pin the nested stock's value so this can't pass vacuously with an
    // un-wired input (`in` defaulting to 0). `grow = in = 2` integrates the
    // nested `out` stock by 2 each of the 5 Euler steps -> 10.
    let results = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;
    let out_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n.ends_with("out"))
        .map(|(_, o)| *o)
        .expect("nested out offset");
    let last = (artifact.layout.n_chunks - 1) * n_slots + out_off;
    assert!(
        (results[last] - 10.0).abs() < 1e-9,
        "nested out stock should integrate to 2*5 = 10, got {}",
        results[last]
    );
}

/// Task 1 (nested stocks under RK4): the same nested-stock submodel under RK4.
/// The recursive stock-offset collection must feed the RK stage math (saved/
/// accum scratch indexed by stock position) the submodel's internal stock, so
/// the four-stage integration covers nested stocks. The wasm must match the VM.
#[test]
fn compile_simulation_submodel_nested_stock_rk4_matches_vm() {
    // A nonlinear flow so RK genuinely differs from Euler: grow = in - out/10,
    // a first-order approach to a steady state, evaluated at trial points.
    let datamodel = submodel_project(
        "nested_stock_rk4",
        crate::datamodel::SimMethod::RungeKutta4,
        "5",
        "in - out / 10",
        true,
        1,
    );
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (nested stock RK4)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare in_value + nested out stock"
    );
}

/// Task 1 (stdlib macro -> implicit module stocks): `SMTH1(input, delay)`
/// expands to a stdlib `smth1` submodule carrying an internal SMOOTH stock.
/// The whole model must match the VM, proving the implicit-module path (the
/// stdlib instance's own `ByteCodeContext`, its nested stock under the RK/Euler
/// loop, and the `EvalModule`/`LoadModuleInput` wiring) reproduces the VM.
/// `SMTH1` was the canonical still-`Skipped` construct before this task.
///
/// A NaN-aware comparison: the stdlib `smth1` instance carries an internal
/// `initial_value` helper slot that is NaN at the t=0 results snapshot in
/// *both* the VM and wasm (it is not written into `curr` before the forced
/// t=0 save), so a finite-difference compare would spuriously fail on a
/// faithful NaN==NaN match. Every user-visible variable (`input`,
/// `smoothed`) is finite and compared exactly.
#[test]
fn compile_simulation_smooth_macro_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("smooth")
        .with_sim_time(0.0, 8.0, 0.25)
        .aux("input", "TIME", None)
        .aux("smoothed", "SMTH1(input, 2)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (SMTH1)");
    // Pin that `smoothed` is finite and nonzero at the last step, so the
    // NaN-aware comparison cannot pass vacuously (an all-NaN `smoothed` would
    // satisfy NaN==NaN). A 2-unit smoothing of `input = TIME` reaches a
    // meaningful positive value by t=8.
    let results = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;
    let smoothed_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "smoothed")
        .map(|(_, o)| *o)
        .expect("smoothed offset");
    let last = (artifact.layout.n_chunks - 1) * n_slots + smoothed_off;
    assert!(
        results[last].is_finite() && results[last] > 0.0,
        "smoothed should be finite and positive by the last step, got {}",
        results[last]
    );
    let checked = assert_matches_vm_nan_aware(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare input + smoothed, only checked {checked}"
    );
}

/// Task 1 (DELAY stdlib macro under RK4): `DELAY3` expands to a stdlib
/// submodule with three chained internal SMOOTH stocks, exercising a deeper
/// nested-stock chain under the RK4 stage math. The wasm must match the VM.
/// NaN-aware for the same internal-`initial_value` reason as the SMTH1 test.
#[test]
fn compile_simulation_delay3_macro_rk4_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("delay3")
        .with_sim_time(0.0, 8.0, 0.25)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .aux("input", "TIME", None)
        .aux("delayed", "DELAY3(input, 2)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (DELAY3 RK4)");
    let checked = assert_matches_vm_nan_aware(sim, &artifact);
    assert!(
        checked >= 2,
        "expected to compare input + delayed, only checked {checked}"
    );
}

/// AC4.1: a host reads the three exported geometry globals from the
/// instantiated module and uses them (no external metadata) to stride one
/// variable's series, which must match the VM.
#[test]
fn compile_simulation_exports_self_describing_geometry() {
    let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
    let mut reader = BufReader::new(file);
    let datamodel = open_xmile(&mut reader).expect("parse population xmile");

    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;

    // Read the three i32 geometry globals straight from the module.
    let read_global = |store: &mut Store<()>, name: &str| -> usize {
        let g = store
            .instance_export(inst, name)
            .unwrap()
            .as_global()
            .unwrap();
        match store.global_read(g) {
            checked::StoredValue::I32(x) => x as usize,
            other => panic!("expected i32 global, got {other:?}"),
        }
    };
    let n_slots = read_global(&mut store, "n_slots");
    let n_chunks = read_global(&mut store, "n_chunks");
    let results_offset = read_global(&mut store, "results_offset");

    // They equal the layout values.
    assert_eq!(n_slots, artifact.layout.n_slots);
    assert_eq!(n_chunks, artifact.layout.n_chunks);
    assert_eq!(results_offset, artifact.layout.results_offset);

    // Stride to the population series using only module-reported geometry.
    let run = store
        .instance_export(inst, "run")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run wasm");
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let pop_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "population")
        .map(|(_, off)| *off)
        .expect("population offset");
    let pop_series: Vec<f64> = store.mem_access_mut_slice(mem, |bytes| {
        (0..n_chunks)
            .map(|c| {
                let a = results_offset + (c * n_slots + pop_off) * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    });

    let mut vm = Vm::new(sim).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    let pop = Ident::<Canonical>::from_str_unchecked("population");
    let vm_pop_off = *vm_results.offsets.get(&pop).expect("vm population offset");
    for (c, &wasm_val) in pop_series.iter().enumerate() {
        let vm_val = vm_results.data[c * vm_results.step_size + vm_pop_off];
        assert!(
            (vm_val - wasm_val).abs() < 1e-9,
            "population mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
        );
    }
}

// ── Array reducers end-to-end (Phase 5 Tasks 1-2) ─────────────────────
//
// These compile real reducer models through the production salsa pipeline
// (so the bytecode is the genuine `PushStaticView; Array<Reduce>; PopView`
// codegen emits, with all constant subscripts baked into the static view)
// and assert the wasm matches the VM. They are the gold-standard parity
// checks for Tasks 1-2; the inline `lower.rs` unit tests pin the individual
// view ops against the VM's addressing oracle.

/// Assert a single scalar variable's wasm series matches the VM, allowing a
/// NaN-vs-NaN match (`assert_matches_vm` rejects NaN via its abs-diff
/// tolerance, so the empty-view / OOB reducers need this NaN-aware variant).
fn assert_scalar_matches_vm(
    sim: std::sync::Arc<CompiledSimulation>,
    artifact: &WasmArtifact,
    name: &str,
) {
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let wasm_data = run_artifact_results(artifact);

    let mut vm = Vm::new(sim).expect("vm creation");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();

    let wasm_off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, off)| *off)
        .unwrap_or_else(|| panic!("{name} not in wasm layout"));
    let ident = Ident::<Canonical>::from_str_unchecked(name);
    let vm_off = *vm_results
        .offsets
        .get(&ident)
        .unwrap_or_else(|| panic!("{name} not in vm offsets"));

    for c in 0..n_chunks {
        let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
        let wasm_val = wasm_data[c * n_slots + wasm_off];
        if vm_val.is_nan() {
            assert!(
                wasm_val.is_nan(),
                "{name} chunk {c}: vm=NaN but wasm={wasm_val}"
            );
        } else {
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
}

/// A 1-D `SUM(source[3:5])` over an indexed dimension: a range subscript that
/// codegen bakes into a static view with `offset=2`, `dims=[3]`. The whole
/// model (including the arrayed `source`) must match the VM.
#[test]
fn compile_simulation_sum_range_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("sum_range")
        .with_sim_time(0.0, 3.0, 1.0)
        .indexed_dimension("A", 5)
        .array_aux("source[A]", "3 * A + 1")
        .scalar_aux("total", "SUM(source[3:5])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (SUM range)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1, "expected to compare source elements + total");
}

/// `SUM(values[*:SubA])` (star-range) selects a sparse subset of a named
/// dimension's elements; codegen bakes the sparse mapping into the static
/// view, exercising the sparse addressing path against the VM. (A transposed
/// reducer like `SUM(matrix')` instead hoists into a `BeginIter` temp-copy
/// loop, so it lands in Phase 5 Task 3; the transpose `ViewDesc` transform
/// itself is pinned by `lower.rs`'s `view_transpose_then_reduce_matches_vm`.)
#[test]
fn compile_simulation_sum_star_range_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("sum_star_range")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("DimA", &["A1", "A2", "A3", "A4"])
        .named_dimension("SubA", &["A2", "A3"])
        .array_with_ranges(
            "values[DimA]",
            vec![("A1", "10"), ("A2", "20"), ("A3", "30"), ("A4", "40")],
        )
        .scalar_aux("total", "SUM(values[*:SubA])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (SUM star range)");
    // The whole model (including the sparse-selected `total` = A2+A3 = 50)
    // matches the VM element-for-element.
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1);
    // Independently pin the sparse selection value against the VM.
    let sim2 = compile_sim(&datamodel, "main");
    assert_scalar_matches_vm(sim2, &artifact, "total");
}

/// A per-element sliced reducer `msum[D] = SUM(m[D, *])` over a 2-D array.
/// Each output element is its own `PushStaticView; ArraySum; PopView` over a
/// per-row static view (the A2A target unrolls to per-element bytecode).
#[test]
fn compile_simulation_sliced_row_sum_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("row_sum")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("D", 2)
        .indexed_dimension("E", 3)
        .array_aux("m[D, E]", "10 * D + E")
        .array_aux("msum[D]", "SUM(m[D, *])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (row sum)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(
        checked >= 1,
        "expected to compare m elements + msum elements"
    );
}

/// MEAN / STDDEV / MAX / MIN / SIZE over a range slice, each matching the VM.
/// One model carries all five so a single compile exercises every reducer's
/// production lowering.
#[test]
fn compile_simulation_all_reducers_match_vm() {
    let datamodel = crate::test_common::TestProject::new("all_reducers")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 5)
        .array_aux("source[A]", "2 * A")
        .scalar_aux("mean_val", "MEAN(source[2:4])")
        .scalar_aux("stddev_val", "STDDEV(source[1:5])")
        .scalar_aux("max_val", "MAX(source[2:4])")
        .scalar_aux("min_val", "MIN(source[2:4])")
        .scalar_aux("size_val", "SIZE(source[2:4])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (all reducers)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 5, "expected to compare all five reducer results");
    for name in ["mean_val", "stddev_val", "max_val", "min_val", "size_val"] {
        assert!(
            artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
            "{name} should be in the layout"
        );
    }
}

// The empty-but-valid view reducer asymmetry (SUM->0.0 vs others->NaN) and
// the invalid-view->NaN-for-all asymmetry are pinned directly against the
// VM's `reduce_view` semantics by the inline `lower.rs` unit tests
// (`empty_valid_view_*` / `invalid_view_*`): a literal empty range
// (`source[4:3]`) is rejected at compile time, and a runtime-empty range
// (`source[start:end]` with `start > end`) plus an out-of-bounds dynamic
// subscript both go through `ViewRangeDynamic` / `ViewSubscriptDynamic`,
// which are Phase 5 Task 4, so the end-to-end coverage of those cases lands
// there.

// ── Phase 5 Task 3: BeginIter iteration loops (end-to-end) ────────────
//
// The broadcasting `LoadIterViewAt` path (source dims != iter dims) is not
// reachable through production codegen (an A2A elementwise op is
// scalar-unrolled, and a mismatched-dim reducer argument fails the engine's
// own dimension check), so it is pinned directly against the VM by
// hand-built-bytecode unit tests in `lower.rs` (`iter_loop_*`). The two
// reachable shapes -- a hoisted same-dim reducer loop and the deferred
// transpose reducer -- are covered end-to-end here.

/// `SUM(2 * source[3:5] + 1)`: the elementwise expression is hoisted into an
/// `AssignTemp` `BeginIter` loop (codegen.rs:1183-1378), then `SUM` reduces
/// the temp. The whole-model wasm must match the VM element-for-element.
#[test]
fn compile_simulation_hoisted_reducer_loop_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("hoist")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 5)
        .array_aux("source[A]", "A")
        .scalar_aux("summed", "SUM(2 * source[3:5] + 1)")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (hoisted reducer)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1, "expected to compare summed");
}

/// `SUM(matrix')`: the transpose materializes the transposed matrix into a
/// temp via a `BeginIter` loop reading the (transposed) source through
/// `LoadIterViewAt`, then sums the temp. This is the case Subcomponent A
/// deferred to the iteration task; the wasm must match the VM.
#[test]
fn compile_simulation_transpose_reducer_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("transpose")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 2)
        .indexed_dimension("B", 3)
        .array_aux("matrix[A,B]", "A * 10 + B")
        .scalar_aux("summed", "SUM(matrix')")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen (transpose)");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1, "expected to compare summed");
}

// ── Phase 5 Task 4: dynamic subscripts + OOB->NaN (end-to-end) ────────

/// Assert every layout variable matches the VM, treating a NaN on both sides
/// as equal (the OOB-subscript result). The plain `assert_matches_vm` uses a
/// finite-difference compare that a NaN would fail, so the OOB tests use this.
fn assert_matches_vm_nan_aware(
    sim: std::sync::Arc<CompiledSimulation>,
    artifact: &WasmArtifact,
) -> usize {
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let wasm_data = run_artifact_results(artifact);
    let mut vm = Vm::new(sim).expect("vm creation");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    assert_eq!(vm_results.step_count, n_chunks, "saved-chunk count differs");

    let mut checked = 0usize;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_data[c * n_slots + *wasm_off];
            if vm_val.is_nan() {
                assert!(
                    wasm_val.is_nan(),
                    "{name} chunk {c}: vm=NaN but wasm={wasm_val}"
                );
            } else {
                let diff = (vm_val - wasm_val).abs();
                assert!(diff < 1e-9, "{name} chunk {c}: vm={vm_val} wasm={wasm_val}");
            }
        }
        checked += 1;
    }
    checked
}

/// Legacy scalar dynamic subscript `arr[idx]` (`PushSubscriptIndex` /
/// `LoadSubscript`), in range: the wasm must match the VM.
#[test]
fn compile_simulation_scalar_dynamic_subscript_in_range_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("dyn")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 4)
        .array_aux("arr[A]", "A * 10")
        .scalar_aux("idx", "3")
        .scalar_aux("picked", "arr[idx]")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1, "expected to compare picked");
}

/// Legacy scalar dynamic subscript `arr[idx]` out of range -> NaN, matching
/// the VM (`vm.rs:1343` sets the subscript invalid, `1361` pushes NaN).
#[test]
fn compile_simulation_scalar_dynamic_subscript_oob_is_nan() {
    // idx = 99 is well past the 4-element dimension -> NaN on both backends.
    let datamodel = crate::test_common::TestProject::new("dyn_oob")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 4)
        .array_aux("arr[A]", "A * 10")
        .scalar_aux("idx", "99")
        .scalar_aux("picked", "arr[idx]")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm_nan_aware(sim, &artifact);
    assert!(checked >= 1, "expected to compare picked");

    // Pin the NaN directly: `picked` must be NaN at every step.
    let n_slots = artifact.layout.n_slots;
    let off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "picked")
        .map(|(_, o)| *o)
        .expect("picked offset");
    let data = run_artifact_results(&artifact);
    for c in 0..artifact.layout.n_chunks {
        assert!(
            data[c * n_slots + off].is_nan(),
            "out-of-bounds arr[idx] must be NaN at chunk {c}"
        );
    }
}

/// `ViewSubscriptDynamic` via `SUM(mat[row, 1])`: a dynamically-subscripted
/// view reduced to a scalar. In range, wasm matches the VM.
#[test]
fn compile_simulation_view_dynamic_subscript_in_range_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("vdyn")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 3)
        .indexed_dimension("B", 4)
        .array_aux("mat[A,B]", "A * 10 + B")
        .scalar_aux("row", "2")
        .scalar_aux("picked", "SUM(mat[row, 1])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm(sim, &artifact);
    assert!(checked >= 1, "expected to compare picked");
}

/// `ViewSubscriptDynamic` out of range -> the view is invalid -> the reducer
/// yields NaN for *all* reducers, matching `reduce_view`'s `if !is_valid`.
#[test]
fn compile_simulation_view_dynamic_subscript_oob_is_nan() {
    let datamodel = crate::test_common::TestProject::new("vdyn_oob")
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("A", 3)
        .indexed_dimension("B", 4)
        .array_aux("mat[A,B]", "A * 10 + B")
        .scalar_aux("row", "99") // out of range for dim A (size 3)
        .scalar_aux("picked", "SUM(mat[row, 1])")
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let checked = assert_matches_vm_nan_aware(sim, &artifact);
    assert!(checked >= 1, "expected to compare picked");

    let n_slots = artifact.layout.n_slots;
    let off = artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == "picked")
        .map(|(_, o)| *o)
        .expect("picked offset");
    let data = run_artifact_results(&artifact);
    for c in 0..artifact.layout.n_chunks {
        assert!(
            data[c * n_slots + off].is_nan(),
            "out-of-bounds SUM(mat[row,1]) must be NaN at chunk {c}"
        );
    }
}

// ── set_value / reset override mechanism (Phase 7 Task 2) ─────────────
//
// An exported `set_value(offset, val) -> i32` writes the override into the
// constants region (0 ok / nonzero when `offset` is not overridable), an
// exported `reset()` resets run state without clearing the region (overrides
// persist across reset, like the VM), and the next `run` re-runs initials +
// the loop sourcing the overridable `AssignConstCurr` from the region.
// `clear_values()` restores compiled defaults. These mirror the VM's
// `set_value_by_offset`/`reset`/`clear_values` (`vm.rs:976-1062`).

/// Instantiate `artifact.wasm`, optionally apply a list of `(offset, value)`
/// overrides via the exported `set_value`, call `reset` then `run`, and copy
/// the step-major results slab out. Each `set_value` return code is checked to
/// be 0 (the caller passes only overridable offsets). Returns the slab.
fn run_artifact_with_overrides(artifact: &WasmArtifact, overrides: &[(usize, f64)]) -> Vec<f64> {
    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let set_value = store
        .instance_export(inst, "set_value")
        .expect("set_value export")
        .as_func()
        .expect("set_value is a function");
    for &(off, val) in overrides {
        let rc: i32 = store
            .invoke_simple_typed::<(i32, f64), i32>(set_value, (off as i32, val))
            .expect("set_value invoke");
        assert_eq!(
            rc, 0,
            "set_value({off}, {val}) should accept an overridable offset"
        );
    }
    let reset = store
        .instance_export(inst, "reset")
        .expect("reset export")
        .as_func()
        .expect("reset is a function");
    store
        .invoke_simple_typed::<(), ()>(reset, ())
        .expect("reset invoke");
    let run = store
        .instance_export(inst, "run")
        .expect("run export")
        .as_func()
        .expect("run is a function");
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run invoke");
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
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

/// Call the exported `set_value` once on a freshly-instantiated module and
/// return its i32 return code, without running the simulation. Used to assert
/// the validation behavior (nonzero on a non-overridable offset).
fn set_value_rc(artifact: &WasmArtifact, off: i32, val: f64) -> i32 {
    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let set_value = store
        .instance_export(inst, "set_value")
        .expect("set_value export")
        .as_func()
        .expect("set_value is a function");
    store
        .invoke_simple_typed::<(i32, f64), i32>(set_value, (off, val))
        .expect("set_value invoke")
}

/// The absolute slab offset of `name` in the artifact's layout.
fn layout_offset(artifact: &WasmArtifact, name: &str) -> usize {
    artifact
        .layout
        .var_offsets
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, o)| *o)
        .unwrap_or_else(|| panic!("{name} offset"))
}

/// The canonical qualified ident for a sub-model `instance`'s sub-variable
/// `var` (`Ident::join`, the U+00B7 module-hierarchy separator), e.g.
/// `sub0·k`. Built the same way `db::layout::flattened_offsets` keys the
/// layout, so it stays correct if the separator ever changes.
fn qualified_ident(instance: &str, var: &str) -> Ident<Canonical> {
    Ident::<Canonical>::join(
        &Ident::<Canonical>::new(instance).as_canonical_str(),
        &Ident::<Canonical>::new(var).as_canonical_str(),
    )
}

/// The absolute slab offsets of the two `submodel_with_constant_project`
/// instances' own constant `k` (`sub0·k`, `sub1·k`). These are distinct
/// because `db::layout::flattened_offsets` bases each instance's keys at the
/// instance's slot, mirroring the VM's `collect_constant_info` recursion.
fn instance_k_offsets(artifact: &WasmArtifact) -> (usize, usize) {
    (
        layout_offset(artifact, qualified_ident("sub0", "k").as_str()),
        layout_offset(artifact, qualified_ident("sub1", "k").as_str()),
    )
}

/// A VM run of `sim` with an override applied at absolute `off` (the VM's
/// `set_value_by_offset`), returning that variable's slab so wasm overrides
/// can be compared cell-for-cell against the VM oracle.
fn vm_results_with_override(
    sim: std::sync::Arc<CompiledSimulation>,
    off: usize,
    val: f64,
) -> (Vec<f64>, usize, usize) {
    let mut vm = Vm::new(sim).expect("vm creation");
    vm.set_value_by_offset(off, val)
        .expect("offset must be a VM-overridable constant");
    vm.run_to_end().expect("vm run");
    let results = vm.into_results();
    (results.data.to_vec(), results.step_size, results.step_count)
}

/// AC5.1: overriding a constant via `set_value`, then `reset`, then `run`,
/// yields the same series the VM produces under the same override. A constant
/// aux feeds a flow that integrates a stock, so the override propagates into
/// every downstream value at every step -- a wrong source (or an override that
/// did not take) would diverge from the VM immediately.
#[test]
fn compile_simulation_set_value_override_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("override")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let rate_off = layout_offset(&artifact, "inflow_rate");
    assert!(
        sim.is_constant_offset(rate_off),
        "inflow_rate must be a VM-overridable constant for this test to be meaningful"
    );

    // Override the constant inflow_rate to 5 (was 2), so level integrates by
    // 5/step: 0,5,10,...,25 -- visibly different from the default 0,2,...,10.
    let wasm_slab = run_artifact_with_overrides(&artifact, &[(rate_off, 5.0)]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let sim_vm = compile_sim(&datamodel, "main");
    let (vm_data, vm_step_size, vm_step_count) = vm_results_with_override(sim_vm, rate_off, 5.0);
    assert_eq!(vm_step_count, n_chunks, "saved-chunk count differs from VM");

    let mut checked = 0usize;
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        // Index the VM slab with the VM's own offset for this variable. It
        // equals `wasm_off` (both backends derive offsets from
        // `db::layout::flattened_offsets`), so this also skips the
        // implicit globals the layout carries but the VM offsets map omits.
        let vm_off = match sim.get_offset(&ident) {
            Some(o) => o,
            None => continue,
        };
        for c in 0..n_chunks {
            let vm_val = vm_data[c * vm_step_size + vm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} mismatch at chunk {c} under override: vm={vm_val} wasm={wasm_val}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected to compare inflow_rate + level + inflow"
    );

    // Pin the override actually took: level reaches 5*5 = 25 (not the default
    // 10), so this cannot pass vacuously with an ignored override.
    let level_off = layout_offset(&artifact, "level");
    let last = (n_chunks - 1) * n_slots + level_off;
    assert!(
        (wasm_slab[last] - 25.0).abs() < 1e-9,
        "level under inflow_rate=5 should reach 25, got {}",
        wasm_slab[last]
    );
}

/// AC5.2: `reset` with no override reproduces the compiled-default series. A
/// `set_value`-then-reset-then-run with an empty override list must match a
/// plain VM run (the default literals), proving the constants region is
/// initialized to the compiled defaults and `reset` leaves them intact.
#[test]
fn compile_simulation_reset_no_override_restores_defaults() {
    let datamodel = crate::test_common::TestProject::new("defaults")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let wasm_slab = run_artifact_with_overrides(&artifact, &[]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    // The default run: level integrates by 2/step -> reaches 10.
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} default mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
    let level_off = layout_offset(&artifact, "level");
    let last = (n_chunks - 1) * n_slots + level_off;
    assert!(
        (wasm_slab[last] - 10.0).abs() < 1e-9,
        "default level should reach 10, got {}",
        wasm_slab[last]
    );
}

/// `set_value` on a non-constant offset returns the error code and does not
/// write. A stock's offset (`level`) is not an overridable constant (its
/// initial is a constant, but it is assigned via `BinOpAssignNext`, not an
/// `AssignConstCurr` in flows), so `set_value` must reject it. After the
/// rejected call the default run must be unchanged.
#[test]
fn compile_simulation_set_value_rejects_non_constant_offset() {
    let datamodel = crate::test_common::TestProject::new("reject")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let level_off = layout_offset(&artifact, "level");
    assert!(
        !sim.is_constant_offset(level_off),
        "level (a stock) must not be a VM-overridable constant"
    );
    // A non-overridable offset returns nonzero.
    assert_ne!(
        set_value_rc(&artifact, level_off as i32, 999.0),
        0,
        "set_value on a stock offset must return a nonzero error code"
    );
    // An out-of-range offset (>= n_slots) also returns nonzero.
    assert_ne!(
        set_value_rc(&artifact, artifact.layout.n_slots as i32, 1.0),
        0,
        "set_value on an out-of-range offset must return a nonzero error code"
    );
    assert_ne!(
        set_value_rc(&artifact, -1, 1.0),
        0,
        "set_value on a negative offset must return a nonzero error code"
    );

    // The rejected write left the constants region untouched: a no-override
    // run still reproduces the defaults (level reaches 10, not 999-driven).
    let wasm_slab = run_artifact_with_overrides(&artifact, &[]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let last = (n_chunks - 1) * n_slots + level_off;
    assert!(
        (wasm_slab[last] - 10.0).abs() < 1e-9,
        "a rejected set_value must not perturb the default run; level should still reach 10, got {}",
        wasm_slab[last]
    );
}

/// `clear_values` restores compiled defaults after an override, without
/// re-instantiating. Override inflow_rate, run (diverges), then clear, reset,
/// run again -- the second run must reproduce the defaults.
#[test]
fn compile_simulation_clear_values_restores_defaults() {
    let datamodel = crate::test_common::TestProject::new("clear")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let rate_off = layout_offset(&artifact, "inflow_rate");
    let level_off = layout_offset(&artifact, "level");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let func = |store: &mut Store<()>, name: &str| {
        store
            .instance_export(inst, name)
            .unwrap()
            .as_func()
            .unwrap()
    };

    // Override -> run -> level reaches 25.
    let set_value = func(&mut store, "set_value");
    let rc: i32 = store
        .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, 5.0))
        .expect("set_value");
    assert_eq!(rc, 0);
    let run = func(&mut store, "run");
    store.invoke_simple_typed::<(), ()>(run, ()).expect("run");

    // clear_values -> reset -> run -> level back to the default 10.
    let clear_values = func(&mut store, "clear_values");
    store
        .invoke_simple_typed::<(), ()>(clear_values, ())
        .expect("clear_values");
    let reset = func(&mut store, "reset");
    store
        .invoke_simple_typed::<(), ()>(reset, ())
        .expect("reset");
    let run = func(&mut store, "run");
    store.invoke_simple_typed::<(), ()>(run, ()).expect("run");

    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let base = artifact.layout.results_offset;
    let last_addr = base + ((n_chunks - 1) * n_slots + level_off) * 8;
    let level_last = store.mem_access_mut_slice(mem, |bytes| {
        f64::from_le_bytes(bytes[last_addr..last_addr + 8].try_into().unwrap())
    });
    assert!(
        (level_last - 10.0).abs() < 1e-9,
        "after clear_values the default level should reach 10, got {level_last}"
    );
}

/// The wasm backend's overridable-constant set (`collect_overridable_defaults`,
/// which mirrors the VM's `collect_constant_info` recursion to capture each
/// default literal) must address EXACTLY the offsets the VM reports overridable
/// via `CompiledSimulation::constant_offsets`. If the two diverged, a blob's
/// `set_value` would accept/reject a different set than the VM's, or initialize
/// the wrong slots -- so this pins them equal over a model with both a top-level
/// constant and a nested-module (SMOOTH) constant.
#[test]
fn wasm_overridable_set_matches_vm_constant_offsets() {
    let datamodel = crate::test_common::TestProject::new("const_set")
        .with_sim_time(0.0, 4.0, 0.5)
        .aux("k", "3", None)
        .aux("input", "TIME + k", None)
        // SMTH1 expands to a nested stdlib module carrying its own constants
        // (the smoothing delay), so the overridable set spans nested modules.
        .aux("smoothed", "SMTH1(input, 2)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");

    let mut wasm_set: Vec<usize> = collect_overridable_defaults(&sim.modules, &sim.root, 0)
        .into_iter()
        .map(|(off, _)| off)
        .collect();
    wasm_set.sort_unstable();
    wasm_set.dedup();

    let mut vm_set: Vec<usize> = sim.constant_offsets().collect();
    vm_set.sort_unstable();

    assert_eq!(
        wasm_set, vm_set,
        "the wasm overridable-constant offsets must match the VM's exactly"
    );
    assert!(
        !vm_set.is_empty(),
        "this model must have at least one overridable constant (k) for the check to be meaningful"
    );

    // Every overridable offset is in range (so it indexes the n_slots-wide
    // const region and the validity byte region safely).
    let n_slots = sim.n_slots();
    for &off in &vm_set {
        assert!(
            off < n_slots,
            "overridable offset {off} must be < n_slots {n_slots}"
        );
    }
}

/// AC5.1 with an override on a constant that feeds an *initial* equation: the
/// VM re-applies the override across initials (it mutates the literal at all
/// locations), so an overridable constant read during the initials phase must
/// also source from the region. Here `seed` is a constant whose value is the
/// stock's initial, so overriding `seed` must change the stock's starting
/// value -- exercising the initials-phase redirect, not just flows.
#[test]
fn compile_simulation_set_value_override_in_initials_matches_vm() {
    let datamodel = crate::test_common::TestProject::new("override_init")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("seed", "5", None)
        .stock("level", "seed", &["hold"], &[], None)
        .flow("hold", "0", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let seed_off = layout_offset(&artifact, "seed");
    assert!(
        sim.is_constant_offset(seed_off),
        "seed must be an overridable constant"
    );

    let wasm_slab = run_artifact_with_overrides(&artifact, &[(seed_off, 42.0)]);
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let sim_vm = compile_sim(&datamodel, "main");
    let (vm_data, vm_step_size, vm_step_count) = vm_results_with_override(sim_vm, seed_off, 42.0);
    assert_eq!(vm_step_count, n_chunks);

    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        if sim.get_offset(&ident).is_none() {
            continue;
        }
        for c in 0..n_chunks {
            let vm_val = vm_data[c * vm_step_size + wasm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} mismatch at chunk {c} under initials override: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
    // seed=42 makes level start (and stay, hold=0) at 42.
    let level_off = layout_offset(&artifact, "level");
    assert!(
        (wasm_slab[level_off] - 42.0).abs() < 1e-9,
        "level should initialize to the overridden seed=42, got {}",
        wasm_slab[level_off]
    );
}

// ── Resumable run ABI (run_initials/run_to) vs the VM oracle ──────────
//
// The blob's persistent step cursor lives in mutable globals
// (`G_SAVED`/`G_STEP_ACCUM`/`G_DID_INITIALS`), so a run can be advanced
// incrementally: `run_initials()` once, then `run_to(t)` per target. The VM
// (`Vm::run_initials`/`run_to`/`reset`/`set_value`) is the correctness oracle
// for every behavior below; the comparator tolerance matches the
// single-shot-`run` tests above (1e-9 cell-for-cell on the in-memory
// fixtures, which run identically on both backends).

/// A small stock + constant-flow fixture with `n_chunks` save points spanning
/// `[0, stop]` at `dt`/`save_step` = 1. `level` integrates `inflow_rate` per
/// step, so a wrong cursor or guard diverges immediately and visibly.
fn resumable_fixture(stop: f64) -> crate::datamodel::Project {
    crate::test_common::TestProject::new("resumable")
        .with_sim_time(0.0, stop, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel()
}

/// Drive the blob's resumable exports on a *fresh* instance: `run_initials`
/// once, then `run_to(t)` for each `t` in `targets`, then copy the whole
/// step-major slab out. The in-module peer of the integration-test helper
/// `run_wasm_results_segmented`; kept here because the lib `#[cfg(test)]`
/// module cannot reach the integration crate's private helpers.
fn run_artifact_segmented(artifact: &WasmArtifact, targets: &[f64]) -> Vec<f64> {
    let info = validate(&artifact.wasm).expect("generated module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run_initials = store
        .instance_export(inst, "run_initials")
        .expect("run_initials export")
        .as_func()
        .expect("run_initials is a function");
    store
        .invoke_simple_typed::<(), ()>(run_initials, ())
        .expect("run_initials wasm");
    for &t in targets {
        let run_to = store
            .instance_export(inst, "run_to")
            .expect("run_to export")
            .as_func()
            .expect("run_to is a function");
        store
            .invoke_simple_typed::<(f64,), ()>(run_to, (t,))
            .expect("run_to wasm");
    }
    read_slab(&mut store, inst, &artifact.layout)
}

/// Copy the whole step-major results slab (`n_chunks * n_slots` f64 at
/// `layout.results_offset`) out of an already-driven instance's `memory`.
fn read_slab(
    store: &mut Store<()>,
    inst: checked::Stored<wasm::addrs::ModuleAddr>,
    layout: &WasmLayout,
) -> Vec<f64> {
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let n = layout.n_chunks * layout.n_slots;
    let base = layout.results_offset;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..n)
            .map(|i| {
                let a = base + i * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Task 1 (AC2.1, AC2.2 foundation): the re-expressed `run`, the resumable
/// `run_initials`+`run_to(stop)`, and the VM must all agree on the full series.
/// `run` is now `reset; run_to(stop)`, so this proves the delegation is
/// faithful (the `run` export matches the segmented drive) and that the
/// resumable path matches the VM (`Vm::run_to_end`) cell-for-cell.
#[test]
fn compile_simulation_run_to_matches_run_and_vm() {
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let stop = sim.specs.stop;

    // (a) the single-shot `run` export.
    let via_run = run_artifact_results(&artifact);
    // (b) run_initials + run_to(stop).
    let via_run_to = run_artifact_segmented(&artifact, &[stop]);
    // (c) the VM oracle.
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    assert_eq!(vm_results.step_count, n_chunks, "VM saved-chunk count");

    // The two wasm paths must be byte-identical (the run re-expression is a
    // pure delegation to run_to, so there is no numeric slack between them).
    assert_eq!(
        via_run, via_run_to,
        "run export diverged from run_initials+run_to(stop) -- the run re-expression is unfaithful"
    );

    // Both wasm paths equal the VM cell-for-cell over every layout variable.
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let run_val = via_run[c * n_slots + wasm_off];
            assert!(
                (vm_val - run_val).abs() < 1e-9,
                "{name} mismatch at chunk {c}: vm={vm_val} wasm={run_val}"
            );
        }
    }

    // AC2.2 foundation: after run_to(t), the saved row for time t holds the
    // VM's value at t. level integrates inflow_rate=2/step from 0, so at t its
    // saved value is 2*t. Drive a fresh instance to t=4 and read level's row 4.
    let level_off = layout_offset(&artifact, "level");
    let to_4 = run_artifact_segmented(&artifact, &[4.0]);
    let mut vm4 = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm4.run_to(4.0).expect("vm run_to(4)");
    let vm4_results = vm4.into_results();
    let vm4_level_off = vm4_results.offsets[&Ident::<Canonical>::from_str_unchecked("level")];
    let wasm_at_4 = to_4[4 * n_slots + level_off];
    let vm_at_4 = vm4_results.data[4 * vm4_results.step_size + vm4_level_off];
    assert!(
        (wasm_at_4 - vm_at_4).abs() < 1e-9 && (wasm_at_4 - 8.0).abs() < 1e-9,
        "level at t=4 after run_to(4): wasm={wasm_at_4} vm={vm_at_4} (expected 8)"
    );
}

/// Read a single f64 slot from the live `curr` chunk (base 0) of an
/// already-driven instance -- the value `run_to` left "current". This is the
/// blob-side analogue of the VM's `get_value_now(off)` (which reads the VM's
/// current chunk): the phase reads AC2.2's "getValue after run_to(t)" at the
/// blob level as the live curr chunk, since the blob has no `getValue` export.
fn read_curr_slot(
    store: &mut Store<()>,
    inst: checked::Stored<wasm::addrs::ModuleAddr>,
    off: usize,
) -> f64 {
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let addr = off * 8; // curr chunk starts at byte 0
    store.mem_access_mut_slice(mem, |bytes| {
        f64::from_le_bytes(bytes[addr..addr + 8].try_into().unwrap())
    })
}

/// The VM's saved slab after driving it through `run_to(t)` for each `t` in
/// `targets` (mirrors `run_artifact_segmented`). `run_to` calls `run_initials`
/// internally, so the VM advances exactly as the blob does.
fn vm_slab_segmented(
    sim: std::sync::Arc<CompiledSimulation>,
    targets: &[f64],
) -> (Vec<f64>, usize, usize) {
    let mut vm = Vm::new(sim).expect("vm creation");
    for &t in targets {
        vm.run_to(t).expect("vm run_to");
    }
    let results = vm.into_results();
    (results.data.to_vec(), results.step_size, results.step_count)
}

/// Count the committed (written) rows in a *blob* results slab after a partial
/// run, via the TIME column. This is sound for the blob specifically because
/// the blob keeps its working `curr`/`next` chunks SEPARATE from the results
/// region (see the "Cursor mapping" caveat): an unwritten results row stays at
/// its zero-initialized state, so its TIME slot reads 0.0 and won't match the
/// expected save-point time `start + c*save_step` for `c > 0`. Row 0 is always
/// written (the forced t=start save). The same heuristic is NOT sound for the
/// VM slab, whose chunk-ring leaks the working chunk into the exported range
/// (its TIME slot holds a genuine overshoot time) -- hence callers derive the
/// VM's committed count analytically instead. Used only with the resumable
/// fixture, where save_step == dt.
fn live_saved_rows(slab: &[f64], n_slots: usize, n_chunks: usize, start: f64, dt: f64) -> usize {
    let mut count = 0usize;
    for c in 0..n_chunks {
        let t = slab[c * n_slots + TIME_OFF];
        let expected = start + c as f64 * dt;
        if c == 0 || (t - expected).abs() < 1e-9 {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Task 2 (AC2.3): a segmented `run_initials; run_to(t1); run_to(t2)` produces
/// the same rows (up to t2) as a single `run_initials; run_to(t2)` and as the
/// VM driven through the same `run_to(t1); run_to(t2)` segments. The cursor in
/// the globals must survive across the two `run_to` calls and resume exactly.
#[test]
fn run_to_segmented_matches_single_and_vm() {
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let (t1, t2) = (3.0, 7.0);
    let segmented = run_artifact_segmented(&artifact, &[t1, t2]);
    let single = run_artifact_segmented(&artifact, &[t2]);
    let (vm_data, vm_step_size, _) = vm_slab_segmented(compile_sim(&datamodel, "main"), &[t1, t2]);

    // Rows for times <= t2 (chunks 0..=7) must agree across all three.
    let last_row = t2 as usize; // save_step == dt == 1, so row index == time.
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(vm_off) = sim.get_offset(&ident) else {
            continue;
        };
        for c in 0..=last_row {
            let seg = segmented[c * n_slots + wasm_off];
            let sng = single[c * n_slots + wasm_off];
            let vm_val = vm_data[c * vm_step_size + vm_off];
            assert!(
                (seg - sng).abs() < 1e-9,
                "{name} segmented vs single mismatch at chunk {c}: {seg} vs {sng}"
            );
            assert!(
                (seg - vm_val).abs() < 1e-9,
                "{name} segmented vs VM mismatch at chunk {c}: {seg} vs {vm_val}"
            );
        }
    }
    assert!(
        n_chunks >= 8,
        "fixture must have at least 8 chunks for t2=7"
    );
}

/// Task 2 (AC2.2): the count of saved rows after `run_to(t)` matches the VM's,
/// for `t` exactly on a save point and `t` between save points.
///
/// Layout note (the phase's "Cursor mapping" caveat): the blob keeps its
/// working `curr`/`next` chunks SEPARATE from the results region, so a partial
/// `run_to(t)` writes exactly the committed save-cadence rows (t=0..floor(t),
/// 5 rows for both t=4 and t=4.5). The VM stores results in a chunk-ring and
/// advances `curr_chunk` THROUGH it, so its working chunk (the t=floor(t)+dt
/// overshoot the guard `curr[TIME] > end` leaves behind) leaks into the
/// exported slab as one extra populated row -- a known chunk-ring artifact, not
/// a committed save point. We therefore compare the committed-save-point count
/// (which both backends agree on) and assert the blob's committed rows equal
/// the VM's on exactly those rows; separately we assert AC2.2 directly: the
/// blob's live `curr` chunk after `run_to(t)` equals the VM's `get_value_now`
/// (which reads the VM's current chunk, i.e. the same t=floor(t)+dt overshoot).
#[test]
fn run_to_at_save_and_between_save_points() {
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let start = sim.specs.start;
    let dt = sim.specs.dt;
    let level_off = layout_offset(&artifact, "level");

    for &t in &[4.0_f64, 4.5_f64] {
        // The committed save-cadence count is analytic from the spec: save
        // points at start + c*save_step (save_step == dt here) for every c with
        // start + c*dt <= t, capped at n_chunks.
        let committed = ((t - start) / dt).floor() as usize + 1;
        let committed = committed.min(n_chunks);

        let slab = run_artifact_segmented(&artifact, &[t]);
        let wasm_rows = live_saved_rows(&slab, n_slots, n_chunks, start, dt);
        assert_eq!(
            wasm_rows, committed,
            "run_to({t}) committed-row count: wasm={wasm_rows}, expected {committed}"
        );
        assert_eq!(committed, 5, "run_to({t}) should commit 5 rows (t=0..4)");

        // The blob's committed rows equal the VM's corresponding rows.
        let (vm_data, vm_step_size, _) = vm_slab_segmented(compile_sim(&datamodel, "main"), &[t]);
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(vm_off) = sim.get_offset(&ident) else {
                continue;
            };
            for c in 0..committed {
                let w = slab[c * n_slots + wasm_off];
                let v = vm_data[c * vm_step_size + vm_off];
                assert!(
                    (w - v).abs() < 1e-9,
                    "{name} committed-row mismatch at chunk {c} (run_to({t})): wasm={w} vm={v}"
                );
            }
        }

        // AC2.2: the blob's live curr chunk (base 0) after run_to(t) equals the
        // VM's get_value_now. Both advance one step past the on-grid target:
        // run_to(4) and run_to(4.5) both leave the cursor at t=5, level=10.
        let info = validate(&artifact.wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let run_initials = store
            .instance_export(inst, "run_initials")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(), ()>(run_initials, ())
            .expect("run_initials");
        let run_to = store
            .instance_export(inst, "run_to")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64,), ()>(run_to, (t,))
            .expect("run_to");
        let curr_level = read_curr_slot(&mut store, inst, level_off);

        let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
        vm.run_to(t).expect("vm run_to");
        let vm_now = vm.get_value_now(
            vm.get_offset(&Ident::<Canonical>::from_str_unchecked("level"))
                .unwrap(),
        );
        assert!(
            (curr_level - vm_now).abs() < 1e-9 && (curr_level - 10.0).abs() < 1e-9,
            "live curr level after run_to({t}): wasm={curr_level} vm={vm_now} (expected 10)"
        );
    }
}

/// Task 2 (AC2.4): `run_to(stop * 2)` clamps to the end -- it equals both a
/// `run_to(stop)` and `Vm::run_to_end`, and saves exactly `n_chunks` rows. The
/// blob clamps via the saved-row exhaustion break (`if saved >= n_chunks`),
/// exactly like the VM's chunk-ring exhaustion: it can never overrun the slab.
#[test]
fn run_to_past_final_time_clamps() {
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let stop = sim.specs.stop;
    let start = sim.specs.start;
    let dt = sim.specs.dt;

    let clamped = run_artifact_segmented(&artifact, &[stop * 2.0]);
    let to_stop = run_artifact_segmented(&artifact, &[stop]);

    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();

    assert_eq!(
        clamped, to_stop,
        "run_to(stop*2) must equal run_to(stop) -- past-FINAL_TIME must clamp"
    );
    // Exactly n_chunks rows are live (the full slab), none beyond.
    assert_eq!(
        live_saved_rows(&clamped, n_slots, n_chunks, start, dt),
        n_chunks,
        "run_to(stop*2) must save exactly n_chunks rows"
    );

    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = clamped[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} clamp mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
}

/// Reconcile #625: after a `run_to(t)` that stops mid-interval, the live curr
/// chunk must be fully self-consistent at the resting time -- every flow / aux /
/// constant evaluated for the same time and stocks `curr` holds -- and
/// identical across the VM and wasm. Previously the integration loop broke
/// right after an advance, so non-stock slots lagged a step (wasm) or held
/// stale garbage including 0 for constants (VM); both backends now re-evaluate
/// root flows once at the resting `curr` after the overshoot break.
#[test]
fn mid_run_curr_is_self_consistent_and_matches_vm() {
    // `doubled = level * 2` is a flow-phase aux that varies every step (it
    // tracks the growing stock), so a one-step flow/aux lag is observable; a
    // constant flow like `inflow` would hide the lag. `prev_level` also covers
    // a PREVIOUS aux mid-run: at the advanced resting point its re-eval reads
    // the last completed step's snapshot, which IS the previous timestep -- so
    // both backends must agree on it too (the `level` of the step before).
    let datamodel = crate::test_common::TestProject::new("midrun")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .aux("doubled", "level * 2", None)
        .aux("prev_level", "PREVIOUS(level, 0)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let level_off = layout_offset(&artifact, "level");
    let doubled_off = layout_offset(&artifact, "doubled");

    // A mid-interval target: run_to(4.5) rests at t=5 (level=10) on both.
    let target = 4.5_f64;

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run_initials = store
        .instance_export(inst, "run_initials")
        .unwrap()
        .as_func()
        .unwrap();
    let run_to = store
        .instance_export(inst, "run_to")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run_initials, ())
        .expect("run_initials");
    store
        .invoke_simple_typed::<(f64,), ()>(run_to, (target,))
        .expect("run_to");

    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to(target).expect("vm run_to");

    // Every variable's live curr value agrees between the two backends -- not
    // just the stocks + reserved time vars, but the flows/auxes/constants too.
    for (name, off) in &artifact.layout.var_offsets {
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(vm_off) = vm.get_offset(&ident) else {
            continue;
        };
        let wasm_val = read_curr_slot(&mut store, inst, *off);
        let vm_val = vm.get_value_now(vm_off);
        assert!(
            (wasm_val - vm_val).abs() < 1e-9,
            "{name} mid-run curr mismatch: wasm={wasm_val} vm={vm_val}"
        );
    }

    // And the curr chunk is internally self-consistent at the resting time:
    // doubled == level * 2 (level=10 at t=5, so doubled=20), not the lagged 16.
    let wasm_level = read_curr_slot(&mut store, inst, level_off);
    let wasm_doubled = read_curr_slot(&mut store, inst, doubled_off);
    assert!(
        (wasm_doubled - wasm_level * 2.0).abs() < 1e-9 && (wasm_doubled - 20.0).abs() < 1e-9,
        "wasm curr not self-consistent: doubled={wasm_doubled} level={wasm_level} (expected 20)"
    );
}

/// Regression (#632 review, P2): the post-loop `flows(0)` re-eval must be
/// SKIPPED after a full / at-stop run, or it corrupts PREVIOUS-using auxes in
/// the live curr chunk. A full run breaks via the slab-exhaustion path, which
/// does NOT advance curr: curr is the just-saved `t=stop` row, and
/// `prev_values` was already snapshotted to that same row (the per-step
/// snapshot runs after the step's flows). A re-eval would then resolve
/// `PREVIOUS(x)` against curr's own snapshot -> `x(stop)` instead of
/// `x(stop-dt)`. Since the wasm host's `getValue` reads the live curr, it would
/// diverge from the committed series and from the VM (which reads the last
/// results row). The `saved < n_chunks` guard skips the re-eval exactly when
/// curr was not advanced (the slab is full), mirroring the VM's
/// `curr_chunk != next_chunk`. Only flows/auxes built on PREVIOUS expose this,
/// so the constant-only teacup parity tests miss it.
#[test]
fn full_run_previous_aux_curr_matches_series_and_vm() {
    // level grows 2/step from 0; prev_level(t) = level(t-dt). At t=stop=5 the
    // correct prev_level is level(4) = 8 -- NOT level(5) = 10.
    let datamodel = crate::test_common::TestProject::new("prev_full")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "rate", None)
        .aux("prev_level", "PREVIOUS(level, 0)", None)
        .build_datamodel();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let prev_off = layout_offset(&artifact, "prev_level");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    // Full run via the single-shot `run` export (reset; run_to(stop)).
    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run = store
        .instance_export(inst, "run")
        .unwrap()
        .as_func()
        .unwrap();
    store.invoke_simple_typed::<(), ()>(run, ()).expect("run");

    // The live curr value (what the host's getValue reads) must equal the last
    // committed series row -- not the re-eval'd self-snapshot.
    let curr_prev = read_curr_slot(&mut store, inst, prev_off);
    let slab = read_slab(&mut store, inst, &artifact.layout);
    let last_series_prev = slab[(n_chunks - 1) * n_slots + prev_off];
    assert!(
        (curr_prev - last_series_prev).abs() < 1e-9,
        "live curr prev_level ({curr_prev}) must equal the last saved row ({last_series_prev})"
    );

    // And both equal the VM's last results row (= level(4) = 8).
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to_end().expect("vm run");
    let vm_results = vm.into_results();
    let vm_off = vm_results.offsets[&Ident::<Canonical>::from_str_unchecked("prev_level")];
    let vm_last = vm_results.data[(vm_results.step_count - 1) * vm_results.step_size + vm_off];
    assert!(
        (curr_prev - vm_last).abs() < 1e-9 && (curr_prev - 8.0).abs() < 1e-9,
        "wasm curr prev_level={curr_prev} vm last={vm_last} (expected 8 = level at t=4)"
    );
}

/// Task 3 (AC3.1, AC5.4): on a single reused instance, `run` then
/// `reset; run` reproduce the same compiled-default series, and both equal the
/// VM (with a `reset` between two VM runs). `reset` clears the cursor globals
/// so the second `run` is a full from-t0 simulation, not a stale resume.
#[test]
fn reset_then_run_reproduces_defaults() {
    let datamodel = resumable_fixture(5.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let invoke_run = |store: &mut Store<()>| {
        let run = store
            .instance_export(inst, "run")
            .unwrap()
            .as_func()
            .unwrap();
        store.invoke_simple_typed::<(), ()>(run, ()).expect("run");
    };
    let invoke_reset = |store: &mut Store<()>| {
        let reset = store
            .instance_export(inst, "reset")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(), ()>(reset, ())
            .expect("reset");
    };

    invoke_run(&mut store);
    let series_a = read_slab(&mut store, inst, &artifact.layout);
    invoke_reset(&mut store);
    invoke_run(&mut store);
    let series_b = read_slab(&mut store, inst, &artifact.layout);

    assert_eq!(
        series_a, series_b,
        "reset; run must reproduce the first run's default series exactly"
    );

    // The VM oracle: a fresh run, then reset, then a second run -- both equal
    // the wasm series.
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to_end().expect("vm run");
    vm.reset();
    vm.run_to_end().expect("vm run after reset");
    let vm_results = vm.into_results();
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = series_b[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} reset-default mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
}

/// Task 3 (AC3.2, AC5.4): `reset` preserves a constant override. On one reused
/// instance: `set_value(inflow_rate, 5)`, `run` -> series A; `reset`, `run` ->
/// series B. A == B (the override survived the reset, since `reset` does not
/// touch the constants region), and both equal the VM run with the same
/// override and a `reset` between runs.
#[test]
fn reset_preserves_overrides() {
    let datamodel = resumable_fixture(5.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let rate_off = layout_offset(&artifact, "inflow_rate");
    assert!(
        sim.is_constant_offset(rate_off),
        "inflow_rate must be an overridable constant"
    );

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    // set_value(inflow_rate, 5) on this instance.
    let set_value = store
        .instance_export(inst, "set_value")
        .unwrap()
        .as_func()
        .unwrap();
    let rc: i32 = store
        .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, 5.0))
        .expect("set_value");
    assert_eq!(rc, 0, "set_value on inflow_rate must succeed");

    let invoke_run = |store: &mut Store<()>| {
        let run = store
            .instance_export(inst, "run")
            .unwrap()
            .as_func()
            .unwrap();
        store.invoke_simple_typed::<(), ()>(run, ()).expect("run");
    };

    invoke_run(&mut store);
    let series_a = read_slab(&mut store, inst, &artifact.layout);
    let reset = store
        .instance_export(inst, "reset")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(reset, ())
        .expect("reset");
    invoke_run(&mut store);
    let series_b = read_slab(&mut store, inst, &artifact.layout);

    assert_eq!(
        series_a, series_b,
        "reset must preserve the override -- both runs use inflow_rate=5"
    );

    // The VM oracle: override, run, reset, run -- the override persists across
    // the VM's reset too (it does not call clear_values).
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.set_value_by_offset(rate_off, 5.0)
        .expect("vm override on a constant");
    vm.run_to_end().expect("vm run");
    vm.reset();
    vm.run_to_end().expect("vm run after reset");
    let vm_results = vm.into_results();
    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = series_b[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} reset-override mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
    // The override actually took: level reaches 5*5 = 25 (not the default 10).
    let level_off = layout_offset(&artifact, "level");
    let last = (n_chunks - 1) * n_slots + level_off;
    assert!(
        (series_b[last] - 25.0).abs() < 1e-9,
        "level under inflow_rate=5 should reach 25 after reset, got {}",
        series_b[last]
    );
}

/// Regression (PR #628 follow-up, P2): the blob owns the live `curr` chunk's
/// override semantics end-to-end, so a host needs no shadow writes into curr.
/// `set_value(off, v)` writes the override into the live curr chunk immediately
/// (mirroring the VM's `set_value_now`, `vm.rs:869-873`), and `reset()`
/// re-establishes the fresh pre-run curr state -- zeroed everywhere except the
/// explicitly-overridden constants, which keep their override (mirroring
/// libsimlin's recreate-and-reapply `simlin_sim_reset`). Previously the blob's
/// set_value/reset left curr untouched and the TS host poked curr directly: a
/// zero-fill on reset that then clobbered the very override it had mirrored.
#[test]
fn set_value_writes_curr_and_reset_reapplies_override() {
    let datamodel = resumable_fixture(5.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let rate_off = layout_offset(&artifact, "inflow_rate");
    let level_off = layout_offset(&artifact, "level");

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let set_value = store
        .instance_export(inst, "set_value")
        .unwrap()
        .as_func()
        .unwrap();
    let reset = store
        .instance_export(inst, "reset")
        .unwrap()
        .as_func()
        .unwrap();
    let clear_values = store
        .instance_export(inst, "clear_values")
        .unwrap()
        .as_func()
        .unwrap();

    // set_value(inflow_rate, 5) on a fresh instance (no run): the override must
    // land in the live curr chunk immediately, not only in the constants region.
    let rc: i32 = store
        .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, 5.0))
        .expect("set_value");
    assert_eq!(rc, 0, "set_value on inflow_rate must succeed");
    assert_eq!(
        read_curr_slot(&mut store, inst, rate_off),
        5.0,
        "set_value must write the override into the live curr chunk"
    );

    // reset(): curr returns to the fresh pre-run state -- the override persists
    // in curr, while every non-overridden slot (the level stock and the reserved
    // TIME slot) reads 0, exactly as a freshly-created VM does after reapply.
    store
        .invoke_simple_typed::<(), ()>(reset, ())
        .expect("reset");
    assert_eq!(
        read_curr_slot(&mut store, inst, rate_off),
        5.0,
        "reset must reapply the explicitly-overridden constant into curr"
    );
    assert_eq!(
        read_curr_slot(&mut store, inst, level_off),
        0.0,
        "reset must zero non-overridden slots in curr"
    );
    assert_eq!(
        read_curr_slot(&mut store, inst, TIME_OFF),
        0.0,
        "reset must zero the reserved time slot in curr"
    );

    // clear_values() drops the override, so a subsequent reset zeroes the slot:
    // a cleared override is no longer reapplied (matching the VM's clear_values).
    store
        .invoke_simple_typed::<(), ()>(clear_values, ())
        .expect("clear_values");
    store
        .invoke_simple_typed::<(), ()>(reset, ())
        .expect("reset after clear_values");
    assert_eq!(
        read_curr_slot(&mut store, inst, rate_off),
        0.0,
        "after clear_values, reset must no longer reapply the dropped override"
    );
}

/// Regression (PR #628 follow-up, P1): a `run_to` that resumes on an
/// already-complete slab (`saved == n_chunks`, reachable via a second
/// `run_to_end` or interactive scrubbing that stays at the end) must be a
/// complete no-op. Previously the stepping loop re-entered -- its
/// `curr[TIME] > target` guard is false when `target >= stop` -- and
/// `emit_save_advance` wrote one results row at `results_base + n_chunks*stride`,
/// one full row past the `n_chunks`-row results region, silently corrupting the
/// snapshot/GF regions that sit immediately after it. The loop now breaks at the
/// top when `saved >= n_chunks`, so a resumed-on-full `run_to` cannot touch
/// linear memory at all.
#[test]
fn run_to_on_full_slab_is_a_noop() {
    // save_step == dt == 1 => save_every == 1, so every step saves and the
    // overshoot row is written immediately on re-entry (the worst case).
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let stop = sim.specs.stop;

    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run_initials = store
        .instance_export(inst, "run_initials")
        .unwrap()
        .as_func()
        .unwrap();
    let run_to = store
        .instance_export(inst, "run_to")
        .unwrap()
        .as_func()
        .unwrap();
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();

    // Fill the slab: run_initials; run_to(stop). saved == n_chunks now.
    store
        .invoke_simple_typed::<(), ()>(run_initials, ())
        .expect("run_initials");
    store
        .invoke_simple_typed::<(f64,), ()>(run_to, (stop,))
        .expect("run_to(stop)");
    let before: Vec<u8> = store.mem_access_mut_slice(mem, |bytes| bytes.to_vec());

    // Resume on the full slab: a re-run to stop, and a run far past it, must each
    // change nothing -- not the results region, not the regions following it.
    store
        .invoke_simple_typed::<(f64,), ()>(run_to, (stop,))
        .expect("run_to(stop) again");
    store
        .invoke_simple_typed::<(f64,), ()>(run_to, (stop * 100.0,))
        .expect("run_to(stop*100)");
    let after: Vec<u8> = store.mem_access_mut_slice(mem, |bytes| bytes.to_vec());

    assert!(
        before == after,
        "run_to on a full slab must be a no-op; linear memory changed (out-of-bounds results write)"
    );
}

/// Task 4 (AC5.3): a mid-run `set_value` affects only steps after the cursor.
/// On one instance: `run_initials; run_to(t1)`, `set_value(inflow_rate, v2)`,
/// `run_to(stop)`. Rows at times <= t1 match a no-override baseline; rows after
/// reflect v2. The whole slab matches the VM driven identically. The override
/// re-reads from the region every step (`lower.rs`'s `AssignConstCurr`
/// redirect), so no new mechanism is needed beyond the resumable run.
///
/// AC5.1 (full-run override) is already covered by
/// `compile_simulation_set_value_override_matches_vm` above; this is the
/// incremental peer.
#[test]
fn mid_run_set_value_matches_vm() {
    let datamodel = resumable_fixture(10.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let n_slots = artifact.layout.n_slots;
    let n_chunks = artifact.layout.n_chunks;
    let stop = sim.specs.stop;
    let rate_off = layout_offset(&artifact, "inflow_rate");
    let level_off = layout_offset(&artifact, "level");
    assert!(sim.is_constant_offset(rate_off));

    let t1 = 5.0_f64;
    let v2 = 5.0_f64;

    // Drive the blob: run_initials; run_to(t1); set_value(rate, v2); run_to(stop).
    let info = validate(&artifact.wasm).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let run_initials = store
        .instance_export(inst, "run_initials")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(), ()>(run_initials, ())
        .expect("run_initials");
    let run_to_t1 = store
        .instance_export(inst, "run_to")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(f64,), ()>(run_to_t1, (t1,))
        .expect("run_to(t1)");
    let set_value = store
        .instance_export(inst, "set_value")
        .unwrap()
        .as_func()
        .unwrap();
    let rc: i32 = store
        .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, v2))
        .expect("set_value");
    assert_eq!(rc, 0, "mid-run set_value on a constant must succeed");
    let run_to_stop = store
        .instance_export(inst, "run_to")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(f64,), ()>(run_to_stop, (stop,))
        .expect("run_to(stop)");
    let wasm_slab = read_slab(&mut store, inst, &artifact.layout);

    // The VM oracle, driven identically.
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    vm.run_to(t1).expect("vm run_to(t1)");
    vm.set_value(&Ident::<Canonical>::from_str_unchecked("inflow_rate"), v2)
        .expect("vm set_value");
    vm.run_to(stop).expect("vm run_to(stop)");
    let vm_results = vm.into_results();

    for (name, wasm_off) in &artifact.layout.var_offsets {
        let wasm_off = *wasm_off;
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let Some(&vm_off) = vm_results.offsets.get(&ident) else {
            continue;
        };
        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_slab[c * n_slots + wasm_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "{name} mid-run mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }

    // Rows up to and including the override-application point match the
    // no-override baseline (rate=2). Because `set_value` runs AFTER
    // `run_to(t1)` -- which (like the VM) advances the committed cursor one
    // step PAST t1, i.e. to t1+dt, running the t1->t1+dt step with the OLD
    // rate -- the first overridden step is t1+dt -> t1+2dt. So rows for
    // t <= t1+dt are unchanged; rows after reflect v2. This is exactly AC5.3's
    // "affects only steps after t1" (the override re-reads from the const
    // region every step, so it cannot retroactively change committed rows).
    let baseline = run_artifact_segmented(&artifact, &[stop]);
    let unchanged_through = (t1 + sim.specs.dt) as usize;
    for c in 0..=unchanged_through {
        let mid = wasm_slab[c * n_slots + level_off];
        let base = baseline[c * n_slots + level_off];
        assert!(
            (mid - base).abs() < 1e-9,
            "level at chunk {c} (<= t1+dt) must match the no-override baseline: mid={mid} base={base}"
        );
    }
    // The override took effect for the later steps: the final committed value
    // exceeds the no-override baseline (rate 5 > 2 after the application point).
    let last = (n_chunks - 1) * n_slots + level_off;
    assert!(
        wasm_slab[last] > baseline[last] + 1.0,
        "level at stop after a mid-run rate bump must exceed the rate=2 baseline: mid={} base={}",
        wasm_slab[last],
        baseline[last]
    );
    // And a row strictly after the application point differs from baseline.
    let after = (unchanged_through + 1) * n_slots + level_off;
    assert!(
        wasm_slab[after] > baseline[after],
        "the first overridden row must exceed the baseline: mid={} base={}",
        wasm_slab[after],
        baseline[after]
    );
}

/// Task 4 (AC5.2): the blob's `set_value` returns nonzero for a non-constant
/// offset (a stock or a computed flow) and zero for an overridable constant.
/// This is the blob-level peer of the VM's `BadOverride` rejection
/// (`vm.rs:1036-1044`); the TS facade turns the nonzero code into a thrown
/// error in Phase 2.
#[test]
fn set_value_nonconstant_returns_error() {
    let datamodel = resumable_fixture(5.0);
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");

    let level_off = layout_offset(&artifact, "level"); // a stock
    let inflow_off = layout_offset(&artifact, "inflow"); // a computed flow
    let rate_off = layout_offset(&artifact, "inflow_rate"); // a constant
    assert!(!sim.is_constant_offset(level_off), "level is a stock");
    assert!(!sim.is_constant_offset(inflow_off), "inflow is computed");
    assert!(
        sim.is_constant_offset(rate_off),
        "inflow_rate is a constant"
    );

    assert_ne!(
        set_value_rc(&artifact, level_off as i32, 1.0),
        0,
        "set_value on a stock offset must return nonzero"
    );
    assert_ne!(
        set_value_rc(&artifact, inflow_off as i32, 1.0),
        0,
        "set_value on a computed-flow offset must return nonzero"
    );
    assert_eq!(
        set_value_rc(&artifact, rate_off as i32, 1.0),
        0,
        "set_value on the overridable constant must return zero"
    );

    // Cross-check the VM rejects the same non-constants and accepts the constant.
    let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
    assert!(
        vm.set_value_by_offset(level_off, 1.0).is_err(),
        "VM must reject a stock offset"
    );
    assert!(
        vm.set_value_by_offset(inflow_off, 1.0).is_err(),
        "VM must reject a computed-flow offset"
    );
    assert!(
        vm.set_value_by_offset(rate_off, 1.0).is_ok(),
        "VM must accept the overridable constant"
    );
}

/// The wasm backend must broadcast a MIXED-SHAPE computed array operand the
/// way the VM does (GH #995).
///
/// The lowering pass that materializes such an operand
/// (`compiler::array_operand`) shapes its temp by the JOIN of the arrays in it,
/// so `vals[d] + matrix[e,d]` iterates over the `[e,d]` shape and reads `vals`
/// broadcast down the rows. Both backends then have to place that narrower
/// source themselves -- the VM through `Opcode::LoadIterViewAt`'s dimension
/// matching, wasm through its own unrolled iteration -- and this is the only
/// row that exercises the disagreement, because the corpus fixture's operands
/// are all single-shaped. Both operand orders run, since the join is the thing
/// making them the same program.
#[test]
fn compile_simulation_mixed_shape_array_operand_matches_vm() {
    for (name, eqn) in [
        (
            "mix_narrow_first",
            "VECTOR SORT ORDER(vals[d] + matrix[e,d], 1)",
        ),
        (
            "mix_wide_first",
            "VECTOR SORT ORDER(matrix[e,d] + vals[d], 1)",
        ),
    ] {
        let datamodel = crate::test_common::TestProject::new(name)
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("d", 3)
            .indexed_dimension("e", 2)
            .array_with_ranges("vals[d]", vec![("1", "30"), ("2", "10"), ("3", "20")])
            .array_with_ranges(
                "matrix[e,d]",
                vec![
                    ("1,1", "1"),
                    ("1,2", "2"),
                    ("1,3", "3"),
                    ("2,1", "10"),
                    ("2,2", "20"),
                    ("2,3", "30"),
                ],
            )
            .array_aux("out[e,d]", eqn)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked > 0, "{name}: no variables compared");
    }
}

/// The wasm twin of
/// `array_operand_materialization_tests::an_array_view_inside_a_module_instance_reads_that_instance`.
///
/// Asserted against ABSOLUTE series rather than through `assert_matches_vm`
/// alone, because parity is exactly what this defect had: `views::ViewDesc`
/// mirrors the VM's addressing arm for arm, so when `PushStaticView` dropped the
/// instance's `module_off` the wasm emitter dropped it too and the two backends
/// agreed on the same wrong numbers. The parity check runs as well -- it is what
/// keeps the two addressing implementations from drifting once both are right.
#[test]
fn compile_simulation_arrayed_submodel_views_address_their_instance() {
    let datamodel = crate::test_common::two_instance_arrayed_submodel_project();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let data = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;

    for (name, expected) in crate::test_common::two_instance_arrayed_submodel_expected() {
        let off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| *o)
            .unwrap_or_else(|| panic!("{name} missing from the wasm layout"));
        for (c, want) in expected.iter().enumerate() {
            let got = data[c * n_slots + off];
            assert!(
                (got - want).abs() < 1e-9,
                "{name} at chunk {c}: expected {want}, got {got}"
            );
        }
    }

    assert_matches_vm(sim, &artifact);
}

/// The wasm twin of
/// `array_operand_materialization_tests::an_array_view_inside_a_nested_module_instance_reads_that_instance`.
///
/// wasm reaches a nested instance by passing `module_off + decl.off` as the
/// child function's param 0, so the two hops must sum there exactly as they do
/// in the VM's recursive `eval`. Asserted against absolute series for the same
/// reason the one-hop wasm pin is.
#[test]
fn compile_simulation_nested_arrayed_submodel_views_address_their_instance() {
    let datamodel = crate::test_common::nested_instance_arrayed_submodel_project();
    let sim = compile_sim(&datamodel, "main");
    let artifact = compile_simulation(&sim).expect("wasm codegen");
    let data = run_artifact_results(&artifact);
    let n_slots = artifact.layout.n_slots;

    for (name, expected) in crate::test_common::nested_instance_arrayed_submodel_expected() {
        let off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| *o)
            .unwrap_or_else(|| panic!("{name} missing from the wasm layout"));
        for (c, want) in expected.iter().enumerate() {
            let got = data[c * n_slots + off];
            assert!(
                (got - want).abs() < 1e-9,
                "{name} at chunk {c}: expected {want}, got {got}"
            );
        }
    }

    assert_matches_vm(sim, &artifact);
}
