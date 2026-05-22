// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! FFI integration tests for `simlin_model_compile_to_wasm`.
//!
//! These exercise the host-facing contract: the function returns a valid wasm
//! blob plus a self-describing, length-prefixed layout buffer (both freeable
//! with `simlin_free`), works from a `SimlinModel` alone (no `SimlinSim`), and
//! surfaces a `SimlinError` -- never a panic -- for a model the wasm backend
//! cannot compile. The blob is validated and executed under the same DLR-FT
//! interpreter the engine's own wasmgen tests use, and the series a host would
//! stride from the results region (using only the returned layout) is checked
//! against the bytecode VM via `simlin_sim_get_series`.

mod common;

use std::ptr;

use checked::{Store, Stored};
use common::open_project_from_datamodel;
use simlin::*;
use simlin_engine::test_common::TestProject;
use wasm::addrs::ModuleAddr;
use wasm::validate;

/// A DLR-FT module instance handle, as returned by `module_instantiate`.
type Inst = Stored<ModuleAddr>;

/// A small scalar stock-and-flow model: a constant inflow fills a stock. Used as
/// the supported-model fixture (it runs through the wasm backend cleanly).
fn simple_model() -> simlin_engine::datamodel::Project {
    TestProject::new("ffi_wasm")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel()
}

/// The host-side layout parse, mirroring the documented little-endian wire
/// format (`n_slots`/`n_chunks`/`results_offset` u64, `count` u32, then per entry
/// `name_len` u32 + UTF-8 name + `offset` u64). Returns the geometry and the
/// name->offset map.
struct ParsedLayout {
    n_slots: usize,
    n_chunks: usize,
    results_offset: usize,
    var_offsets: Vec<(String, usize)>,
}

fn parse_layout(bytes: &[u8]) -> ParsedLayout {
    let mut pos = 0usize;
    let read_u64 = |pos: &mut usize| -> u64 {
        let v = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        v
    };
    let read_u32 = |pos: &mut usize| -> u32 {
        let v = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        v
    };
    let n_slots = read_u64(&mut pos) as usize;
    let n_chunks = read_u64(&mut pos) as usize;
    let results_offset = read_u64(&mut pos) as usize;
    let count = read_u32(&mut pos) as usize;
    let mut var_offsets = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = read_u32(&mut pos) as usize;
        let name = String::from_utf8(bytes[pos..pos + name_len].to_vec()).unwrap();
        pos += name_len;
        let offset = read_u64(&mut pos) as usize;
        var_offsets.push((name, offset));
    }
    assert_eq!(pos, bytes.len(), "layout buffer had trailing bytes");
    ParsedLayout {
        n_slots,
        n_chunks,
        results_offset,
        var_offsets,
    }
}

/// AC6.1: `simlin_model_compile_to_wasm` returns a valid wasm blob plus the
/// name->offset layout via the malloc-return convention; both buffers free with
/// `simlin_free`; it works from a `SimlinModel` with no `SimlinSim`.
#[test]
fn compile_to_wasm_returns_blob_and_layout() {
    let datamodel = simple_model();
    unsafe {
        let project = open_project_from_datamodel(&datamodel);
        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        // No SimlinSim is ever created -- the model handle alone must suffice.
        let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
        assert!(err.is_null(), "get_model should not error");
        assert!(!model.is_null(), "model handle must be non-null");

        let mut out_wasm: *mut u8 = ptr::null_mut();
        let mut out_wasm_len: usize = 0;
        let mut out_layout: *mut u8 = ptr::null_mut();
        let mut out_layout_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_model_compile_to_wasm(
            model,
            &mut out_wasm,
            &mut out_wasm_len,
            &mut out_layout,
            &mut out_layout_len,
            &mut err,
        );
        assert!(
            err.is_null(),
            "compile_to_wasm should not error on a supported model"
        );
        assert!(
            !out_wasm.is_null() && out_wasm_len > 0,
            "wasm blob must be non-empty"
        );
        assert!(
            !out_layout.is_null() && out_layout_len > 0,
            "layout buffer must be non-empty"
        );

        // The wasm blob validates under the interpreter.
        let wasm = std::slice::from_raw_parts(out_wasm, out_wasm_len).to_vec();
        validate(&wasm).expect("returned blob must validate");

        // The layout deserializes to the expected geometry + name->offset map.
        let layout_bytes = std::slice::from_raw_parts(out_layout, out_layout_len).to_vec();
        let layout = parse_layout(&layout_bytes);
        assert!(
            layout.n_slots >= 4,
            "scalar model has at least the 4 reserved slots"
        );
        // dt=1 over [0,10] -> 11 saved samples.
        assert_eq!(layout.n_chunks, 11, "n_chunks should match the sim specs");
        // The results region sits two chunks past the start of memory (curr+next).
        assert_eq!(
            layout.results_offset,
            2 * layout.n_slots * 8,
            "results_offset = 2 chunks (curr + next) past byte 0"
        );
        for name in ["level", "inflow", "inflow_rate"] {
            assert!(
                layout.var_offsets.iter().any(|(n, _)| n == name),
                "{name} must appear in the layout name->offset map"
            );
        }
        // Offsets are within a chunk.
        for (name, off) in &layout.var_offsets {
            assert!(
                *off < layout.n_slots,
                "{name} offset {off} must be < n_slots"
            );
        }

        // Run the blob and stride `level`'s series using only the layout, then
        // check it against the VM's series.
        let level_off = layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "level")
            .map(|(_, o)| *o)
            .unwrap();
        let blob_level = run_and_stride(&wasm, &layout, level_off);
        // level integrates by 2/step: 0, 2, 4, ..., 20.
        assert!((blob_level[0]).abs() < 1e-9, "level starts at 0");
        assert!(
            (blob_level[blob_level.len() - 1] - 20.0).abs() < 1e-9,
            "level reaches 20 by the last step, got {}",
            blob_level[blob_level.len() - 1]
        );
        let vm_level = vm_series(project, &model_name, "level", layout.n_chunks);
        assert_eq!(blob_level.len(), vm_level.len());
        for (c, (&b, &v)) in blob_level.iter().zip(vm_level.iter()).enumerate() {
            assert!((b - v).abs() < 1e-9, "level chunk {c}: blob {b} != vm {v}");
        }

        // Both buffers free with simlin_free without leaking or double-free.
        simlin_free(out_wasm);
        simlin_free(out_layout);

        simlin_model_unref(model);
        simlin_project_unref(project);
    }
}

/// engine-wasm-sim.AC2.3 + AC5.3 across the `simlin_model_compile_to_wasm` path:
/// the blob compiled via the FFI carries and honors the resumable ABI
/// (`run_initials`/`run_to`/`reset`) added in Subcomponent A. The FFI signature
/// itself is unchanged -- the resumable surface is reached purely through the
/// blob's own exports.
///
/// Both the blob and the bytecode-VM oracle are driven through the *same*
/// segmented sequence: advance to `t1`, override the constant `inflow_rate`
/// mid-run, then advance to the end. Because a mid-run constant override is
/// re-read each step (it affects only steps after `t1`), and because we compare
/// the complete end-of-run `level` series (not a partial-run intermediate slab,
/// which can differ by the VM's one leaked working chunk), the two must agree
/// exactly here.
#[test]
fn compile_to_wasm_blob_supports_resumable_run() {
    let datamodel = simple_model();
    // t1 lands on a save point; the override raises inflow_rate partway through.
    let t1 = 5.0;
    let stop = 10.0;
    let override_val = 5.0;
    unsafe {
        let project = open_project_from_datamodel(&datamodel);
        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
        assert!(err.is_null(), "get_model should not error");
        assert!(!model.is_null(), "model handle must be non-null");

        let mut out_wasm: *mut u8 = ptr::null_mut();
        let mut out_wasm_len: usize = 0;
        let mut out_layout: *mut u8 = ptr::null_mut();
        let mut out_layout_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_model_compile_to_wasm(
            model,
            &mut out_wasm,
            &mut out_wasm_len,
            &mut out_layout,
            &mut out_layout_len,
            &mut err,
        );
        assert!(err.is_null(), "compile_to_wasm should not error");
        assert!(
            !out_wasm.is_null() && out_wasm_len > 0,
            "blob must be non-empty"
        );

        let wasm = std::slice::from_raw_parts(out_wasm, out_wasm_len).to_vec();
        validate(&wasm).expect("returned blob must validate");
        let layout_bytes = std::slice::from_raw_parts(out_layout, out_layout_len).to_vec();
        let layout = parse_layout(&layout_bytes);

        // The export-set growth is purely additive: the blob still carries every
        // original export at its original kind, plus the two new resumable funcs.
        assert_blob_exports(&wasm);

        let level_off = layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "level")
            .map(|(_, o)| *o)
            .expect("level must be in the layout");
        let inflow_rate_off = layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "inflow_rate")
            .map(|(_, o)| *o)
            .expect("inflow_rate must be in the layout");

        // Drive the blob's resumable ABI on ONE instance: run_initials ->
        // run_to(t1) -> set_value(inflow_rate) -> run_to(stop), reading level's
        // strided series at the end. The same instance is then reset and re-run.
        let info = validate(&wasm).expect("validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;

        invoke_unit(&mut store, inst, "run_initials");
        invoke_run_to(&mut store, inst, t1);
        let rc = invoke_set_value(&mut store, inst, inflow_rate_off as i32, override_val);
        assert_eq!(rc, 0, "set_value on the overridable constant must return 0");
        invoke_run_to(&mut store, inst, stop);
        let blob_segmented = stride_var(&store, inst, &layout, level_off);

        // VM oracle driven identically through the FFI: new -> run_to(t1) ->
        // set_value -> run_to_end -> get_series.
        let vm_segmented = vm_series_segmented_override(
            project,
            &model_name,
            "level",
            "inflow_rate",
            t1,
            override_val,
            layout.n_chunks,
        );
        assert_eq!(
            blob_segmented.len(),
            vm_segmented.len(),
            "blob and VM series length must match"
        );
        for (c, (&b, &v)) in blob_segmented.iter().zip(vm_segmented.iter()).enumerate() {
            assert!(
                (b - v).abs() < 1e-9,
                "segmented level chunk {c}: blob {b} != vm {v}"
            );
        }

        // reset across the FFI compile path: the override survives reset (the
        // const-override region is untouched), so a fresh full `run` on the SAME
        // instance reproduces the override-applied defaults -- a from-t0 run with
        // inflow_rate = override_val throughout. Peer of `simlin_sim_reset`.
        invoke_unit(&mut store, inst, "reset");
        invoke_unit(&mut store, inst, "run");
        let blob_after_reset = stride_var(&store, inst, &layout, level_off);

        let vm_override_full = vm_series_with_override(
            project,
            &model_name,
            "level",
            "inflow_rate",
            override_val,
            layout.n_chunks,
        );
        assert_eq!(
            blob_after_reset.len(),
            vm_override_full.len(),
            "post-reset blob and VM series length must match"
        );
        for (c, (&b, &v)) in blob_after_reset
            .iter()
            .zip(vm_override_full.iter())
            .enumerate()
        {
            assert!(
                (b - v).abs() < 1e-9,
                "post-reset level chunk {c}: blob {b} != vm {v}"
            );
        }
        // The override raised every step relative to the unmodified defaults, so
        // the post-reset run is genuinely the override-applied series, not the
        // compiled default (a guard against reset silently clearing overrides).
        assert!(
            (blob_after_reset[blob_after_reset.len() - 1] - 50.0).abs() < 1e-9,
            "with inflow_rate={override_val} throughout, level reaches 50, got {}",
            blob_after_reset[blob_after_reset.len() - 1]
        );

        simlin_free(out_wasm);
        simlin_free(out_layout);
        simlin_model_unref(model);
        simlin_project_unref(project);
    }
}

/// AC6.2: a model the wasm backend cannot compile surfaces a `SimlinError`
/// (out_error is set, both buffers stay NULL), never a panic across the FFI
/// boundary. `SUM(source[lo:hi])` with variable bounds lowers to a runtime view
/// range the fully-unrolled emitter cannot express.
#[test]
fn compile_to_wasm_unsupported_model_surfaces_error() {
    let datamodel = TestProject::new("ffi_wasm_unsupported")
        .with_sim_time(0.0, 5.0, 1.0)
        .indexed_dimension("A", 5)
        .array_aux("source[A]", "A")
        .scalar_aux("lo", "2")
        .scalar_aux("hi", "4")
        .scalar_aux("total", "SUM(source[lo:hi])")
        .build_datamodel();
    unsafe {
        let project = open_project_from_datamodel(&datamodel);
        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let mut out_wasm: *mut u8 = ptr::null_mut();
        let mut out_wasm_len: usize = 0;
        let mut out_layout: *mut u8 = ptr::null_mut();
        let mut out_layout_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_model_compile_to_wasm(
            model,
            &mut out_wasm,
            &mut out_wasm_len,
            &mut out_layout,
            &mut out_layout_len,
            &mut err,
        );

        assert!(!err.is_null(), "an unsupported model must set out_error");
        // The message names the unsupported construct (no panic, a clean error).
        let msg_ptr = simlin_error_get_message(err);
        assert!(!msg_ptr.is_null(), "the error must carry a message");
        let msg = std::ffi::CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("ViewRangeDynamic") || msg.contains("code generation failed"),
            "error message should describe the codegen failure, got: {msg}"
        );
        // Both output buffers stay NULL on failure.
        assert!(
            out_wasm.is_null() && out_wasm_len == 0,
            "wasm buffer stays NULL on error"
        );
        assert!(
            out_layout.is_null() && out_layout_len == 0,
            "layout buffer stays NULL on error"
        );

        simlin_error_free(err);
        simlin_model_unref(model);
        simlin_project_unref(project);
    }
}

/// NULL output pointers are rejected with an error rather than a crash.
#[test]
fn compile_to_wasm_null_outputs_error() {
    let datamodel = simple_model();
    unsafe {
        let project = open_project_from_datamodel(&datamodel);
        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
        assert!(!model.is_null());

        let mut out_wasm: *mut u8 = ptr::null_mut();
        let mut out_wasm_len: usize = 0;
        let mut out_layout_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        // A NULL out_layout pointer must be rejected.
        simlin_model_compile_to_wasm(
            model,
            &mut out_wasm,
            &mut out_wasm_len,
            ptr::null_mut(),
            &mut out_layout_len,
            &mut err,
        );
        assert!(!err.is_null(), "a NULL output pointer must set out_error");
        simlin_error_free(err);

        simlin_model_unref(model);
        simlin_project_unref(project);
    }
}

/// Instantiate `wasm` under the interpreter, invoke `run`, and stride out the
/// `n_chunks`-long series for the variable at `off` (using only the layout).
fn run_and_stride(wasm: &[u8], layout: &ParsedLayout, off: usize) -> Vec<f64> {
    let info = validate(wasm).expect("validate");
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
    let mem = store
        .instance_export(inst, "memory")
        .unwrap()
        .as_mem()
        .unwrap();
    let base = layout.results_offset;
    let n_slots = layout.n_slots;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..layout.n_chunks)
            .map(|c| {
                let a = base + (c * n_slots + off) * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Assert the FFI-compiled blob carries every original export (at its original
/// kind) plus the two resumable functions added in Subcomponent A. The original
/// set is `run`/`set_value`/`reset`/`clear_values` (funcs), `memory`, and the
/// geometry globals `n_slots`/`n_chunks`/`results_offset`; the additions are
/// `run_to`/`run_initials`. This pins the export-set growth as purely additive.
fn assert_blob_exports(wasm: &[u8]) {
    let info = validate(wasm).expect("validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    for name in [
        "run",
        "set_value",
        "reset",
        "clear_values",
        "run_to",
        "run_initials",
    ] {
        let exp = store
            .instance_export(inst, name)
            .unwrap_or_else(|_| panic!("blob must export `{name}`"));
        assert!(
            exp.as_func().is_some(),
            "export `{name}` must be a function"
        );
    }
    assert!(
        store
            .instance_export(inst, "memory")
            .expect("blob must export `memory`")
            .as_mem()
            .is_some(),
        "export `memory` must be a memory"
    );
    for name in ["n_slots", "n_chunks", "results_offset"] {
        let exp = store
            .instance_export(inst, name)
            .unwrap_or_else(|_| panic!("blob must export `{name}`"));
        assert!(
            exp.as_global().is_some(),
            "export `{name}` must be a global"
        );
    }
}

/// Invoke a `() -> ()` blob export (`run_initials`/`run`/`reset`) on `inst`.
fn invoke_unit(store: &mut Store<()>, inst: Inst, name: &str) {
    let f = store
        .instance_export(inst, name)
        .unwrap_or_else(|_| panic!("`{name}` export must exist"))
        .as_func()
        .unwrap_or_else(|| panic!("`{name}` export must be a function"));
    store
        .invoke_simple_typed::<(), ()>(f, ())
        .unwrap_or_else(|_| panic!("invoke `{name}`"));
}

/// Invoke `run_to(target)` (a `(f64) -> ()` export) on `inst`.
fn invoke_run_to(store: &mut Store<()>, inst: Inst, target: f64) {
    let f = store
        .instance_export(inst, "run_to")
        .expect("`run_to` export must exist")
        .as_func()
        .expect("`run_to` export must be a function");
    store
        .invoke_simple_typed::<(f64,), ()>(f, (target,))
        .expect("invoke `run_to`");
}

/// Invoke `set_value(offset, val)` (a `(i32, f64) -> i32` export) on `inst`,
/// returning the blob's status code (0 = applied, nonzero = rejected).
fn invoke_set_value(store: &mut Store<()>, inst: Inst, offset: i32, val: f64) -> i32 {
    let f = store
        .instance_export(inst, "set_value")
        .expect("`set_value` export must exist")
        .as_func()
        .expect("`set_value` export must be a function");
    store
        .invoke_simple_typed::<(i32, f64), i32>(f, (offset, val))
        .expect("invoke `set_value`")
}

/// Stride the `n_chunks`-long series for the variable at `off` out of the live
/// instance's results region (using only the layout geometry), without
/// re-invoking `run` -- the cursor/run state already lives in the instance.
fn stride_var(store: &Store<()>, inst: Inst, layout: &ParsedLayout, off: usize) -> Vec<f64> {
    let mem = store
        .instance_export(inst, "memory")
        .expect("`memory` export must exist")
        .as_mem()
        .expect("`memory` export must be a memory");
    let base = layout.results_offset;
    let n_slots = layout.n_slots;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..layout.n_chunks)
            .map(|c| {
                let a = base + (c * n_slots + off) * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// VM oracle for a segmented, mid-run-override drive through the FFI:
/// `simlin_sim_new` -> `run_to(t1)` -> `set_value(const_name, v)` ->
/// `run_to_end` -> `get_series(name)`. Mirrors the blob's resumable sequence.
unsafe fn vm_series_segmented_override(
    project: *mut SimlinProject,
    model_name: &std::ffi::CStr,
    name: &str,
    const_name: &str,
    t1: f64,
    override_val: f64,
    n_chunks: usize,
) -> Vec<f64> {
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
    assert!(err.is_null());
    let sim = simlin_sim_new(model, false, &mut err);
    assert!(err.is_null(), "sim_new should succeed");

    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_run_to(sim, t1, &mut err);
    assert!(err.is_null(), "run_to(t1) should succeed");

    let const_c = std::ffi::CString::new(const_name).unwrap();
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_set_value(sim, const_c.as_ptr(), override_val, &mut err);
    assert!(err.is_null(), "set_value on a constant should succeed");

    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null(), "run_to_end should succeed");

    let series = read_series(sim, name, n_chunks);
    simlin_sim_unref(sim);
    simlin_model_unref(model);
    series
}

/// VM oracle for a full from-t0 run with a constant override applied before the
/// run: `simlin_sim_new` -> `set_value(const_name, v)` -> `run_to_end` ->
/// `get_series(name)`. This is the "override-applied defaults" the blob must
/// reproduce after a `reset` that preserves overrides.
unsafe fn vm_series_with_override(
    project: *mut SimlinProject,
    model_name: &std::ffi::CStr,
    name: &str,
    const_name: &str,
    override_val: f64,
    n_chunks: usize,
) -> Vec<f64> {
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
    assert!(err.is_null());
    let sim = simlin_sim_new(model, false, &mut err);
    assert!(err.is_null(), "sim_new should succeed");

    let const_c = std::ffi::CString::new(const_name).unwrap();
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_set_value(sim, const_c.as_ptr(), override_val, &mut err);
    assert!(err.is_null(), "set_value on a constant should succeed");

    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null(), "run_to_end should succeed");

    let series = read_series(sim, name, n_chunks);
    simlin_sim_unref(sim);
    simlin_model_unref(model);
    series
}

/// Read `name`'s series from a run sim via `simlin_sim_get_series`, truncated to
/// the number actually written.
unsafe fn read_series(sim: *mut SimlinSim, name: &str, n_chunks: usize) -> Vec<f64> {
    let name_c = std::ffi::CString::new(name).unwrap();
    let mut results = vec![0.0f64; n_chunks];
    let mut written: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_series(
        sim,
        name_c.as_ptr(),
        results.as_mut_ptr(),
        n_chunks,
        &mut written,
        &mut err,
    );
    assert!(err.is_null(), "get_series should succeed");
    results.truncate(written);
    results
}

/// The VM's series for `name` via `simlin_sim_new` + `simlin_sim_get_series`.
unsafe fn vm_series(
    project: *mut SimlinProject,
    model_name: &std::ffi::CStr,
    name: &str,
    n_chunks: usize,
) -> Vec<f64> {
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
    assert!(err.is_null());
    let sim = simlin_sim_new(model, false, &mut err);
    assert!(
        err.is_null(),
        "sim_new should succeed for a supported model"
    );
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null(), "run_to_end should succeed");

    let name_c = std::ffi::CString::new(name).unwrap();
    let mut results = vec![0.0f64; n_chunks];
    let mut written: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_series(
        sim,
        name_c.as_ptr(),
        results.as_mut_ptr(),
        n_chunks,
        &mut written,
        &mut err,
    );
    assert!(err.is_null(), "get_series should succeed");
    results.truncate(written);

    simlin_sim_unref(sim);
    simlin_model_unref(model);
    results
}
