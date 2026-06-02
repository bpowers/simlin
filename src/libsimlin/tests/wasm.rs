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
            false,
            false,
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
            false,
            false,
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
            false,
            false,
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

/// wasm-ltm.AC3.1 (FFI side): an LTM-enabled compile of an unlowerable model
/// returns a clean `SimlinError` with both output buffers NULL and never
/// panics across the FFI boundary. The fixture combines a real feedback loop
/// (so LTM is genuinely enabled and link/loop scores would be emitted on the
/// VM path) with `SUM(source[lo:hi])` -- the dynamic-range subscript the
/// fully-unrolled emitter cannot express (GH #612). Loaded from a real XMILE
/// file rather than a `TestProject` builder so the same fixture serves the
/// engine-level twin (`unsupported_ltm_model_returns_wasmgen_error` in
/// `simulate_ltm_wasm.rs`) and the TS twin in `wasm-ltm.test.ts`.
#[test]
fn compile_to_wasm_unsupported_ltm_model_surfaces_error() {
    let path = std::path::Path::new("../../test/ltm_dynamic_range_unsupported/model.stmx");
    let stmx =
        std::fs::read(path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()));
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let project = simlin_project_open_xmile(stmx.as_ptr(), stmx.len(), &mut err);
        assert!(err.is_null(), "open_xmile must succeed for a valid XMILE");
        assert!(!project.is_null());

        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
        assert!(err.is_null(), "get_model must succeed");
        assert!(!model.is_null());

        let mut out_wasm: *mut u8 = ptr::null_mut();
        let mut out_wasm_len: usize = 0;
        let mut out_layout: *mut u8 = ptr::null_mut();
        let mut out_layout_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        // Phase 1's 8-arg signature with the LTM flag set: this is the path
        // an LTM-on-wasm caller takes, and the failure must surface cleanly
        // here -- not panic across the FFI boundary and not silently fall
        // back to a non-LTM blob.
        simlin_model_compile_to_wasm(
            model,
            /* ltm_enabled */ true,
            /* ltm_discovery_mode */ false,
            &mut out_wasm,
            &mut out_wasm_len,
            &mut out_layout,
            &mut out_layout_len,
            &mut err,
        );

        assert!(
            !err.is_null(),
            "an unsupported LTM model must populate out_error (no panic, no silent success)"
        );
        let msg_ptr = simlin_error_get_message(err);
        assert!(!msg_ptr.is_null(), "the error must carry a message");
        let msg = std::ffi::CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("ViewRangeDynamic") || msg.contains("code generation failed"),
            "error message should describe the codegen failure, got: {msg}"
        );

        // Both output buffers stay NULL on failure -- no half-populated blob,
        // no layout-without-blob (or vice versa) the caller could misread.
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
            false,
            false,
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
/// `run_to`/`run_initials` (funcs) and `saved_steps` (the live saved-row counter
/// global). This pins the export-set growth as purely additive.
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
    for name in ["n_slots", "n_chunks", "results_offset", "saved_steps"] {
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

// ── from-wasm analyze parity (phase 2 subcomponent B, tasks 4 & 5) ─────────

/// Read the logistic-growth-LTM fixture from the repo `test/` tree.  This is
/// a small scalar LTM model -- 1 stock, 1 flow, 4 auxes, one reinforcing
/// feedback loop -- so it exercises the full link-score + rel-loop-score
/// path without the surface area an arrayed model would add (which Phase 4
/// covers).
fn read_logistic_growth_stmx() -> Vec<u8> {
    let path = std::path::Path::new("../../test/logistic_growth_ltm/logistic_growth.stmx");
    std::fs::read(path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
}

/// Run the LTM-enabled wasm blob and lift out its full result slab (the
/// `n_chunks * n_slots` f64 region starting at `results_offset` in the
/// blob's linear memory).  Mirrors the `run_and_stride` pattern but returns
/// every variable's column rather than one variable's series.
fn run_and_extract_slab(wasm: &[u8], layout: &ParsedLayout) -> Vec<u8> {
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
    let n_bytes = layout.n_chunks * layout.n_slots * 8;
    store.mem_access_mut_slice(mem, |bytes| bytes[base..base + n_bytes].to_vec())
}

/// Container holding everything a parity test needs after compiling +
/// running the wasm blob with LTM enabled.  The held buffers are freed by
/// `Drop`.
struct WasmRun {
    project: *mut SimlinProject,
    model: *mut SimlinModel,
    layout_bytes_ptr: *mut u8,
    layout_bytes_len: usize,
    wasm_bytes_ptr: *mut u8,
    slab_bytes: Vec<u8>,
}

impl Drop for WasmRun {
    fn drop(&mut self) {
        unsafe {
            simlin_free(self.wasm_bytes_ptr);
            simlin_free(self.layout_bytes_ptr);
            simlin_model_unref(self.model);
            simlin_project_unref(self.project);
        }
    }
}

/// Compile the logistic-growth LTM model to a wasm blob (with `ltm_enabled =
/// true`), run it under the DLR-FT interpreter, and return both the raw
/// blob outputs (held for the FFI parity calls) and the strided slab the
/// host extracted out of linear memory.
unsafe fn compile_and_run_logistic_growth_ltm() -> WasmRun {
    let stmx = read_logistic_growth_stmx();
    let mut err: *mut SimlinError = ptr::null_mut();
    let project = simlin_project_open_xmile(stmx.as_ptr(), stmx.len(), &mut err);
    assert!(err.is_null(), "open_xmile must succeed");
    assert!(!project.is_null());

    let model_name = std::ffi::CString::new("main").unwrap();
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
    assert!(err.is_null(), "get_model must succeed");
    assert!(!model.is_null());

    let mut out_wasm: *mut u8 = ptr::null_mut();
    let mut out_wasm_len: usize = 0;
    let mut out_layout: *mut u8 = ptr::null_mut();
    let mut out_layout_len: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    // Compile with the new ABI flag carried by Phase 1's signature change:
    // `ltm_enabled = true`, `ltm_discovery_mode = false`.
    simlin_model_compile_to_wasm(
        model,
        true,
        false,
        &mut out_wasm,
        &mut out_wasm_len,
        &mut out_layout,
        &mut out_layout_len,
        &mut err,
    );
    assert!(
        err.is_null(),
        "compile_to_wasm(ltm_enabled=true) must succeed for the scalar LTM model"
    );
    assert!(!out_wasm.is_null() && out_wasm_len > 0);
    assert!(!out_layout.is_null() && out_layout_len > 0);

    let wasm_slice = std::slice::from_raw_parts(out_wasm, out_wasm_len).to_vec();
    let layout_slice = std::slice::from_raw_parts(out_layout, out_layout_len).to_vec();
    let parsed = parse_layout(&layout_slice);
    let slab_bytes = run_and_extract_slab(&wasm_slice, &parsed);

    let _ = parsed; // parsed only needed to compute slab; layout buffer is what FFI consumes
    WasmRun {
        project,
        model,
        layout_bytes_ptr: out_layout,
        layout_bytes_len: out_layout_len,
        wasm_bytes_ptr: out_wasm,
        slab_bytes,
    }
}

/// Run the VM oracle on the same logistic-growth model with LTM enabled and
/// return the FFI `SimlinLinks*` from `simlin_analyze_get_links`.  Caller
/// frees with `simlin_free_links`.  Borrows the already-opened project so
/// it can share the project handle with the from-wasm caller (and therefore
/// the same db / sync state).
unsafe fn vm_oracle_links(project: *mut SimlinProject) -> *mut SimlinLinks {
    let model_name = std::ffi::CString::new("main").unwrap();
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(project, model_name.as_ptr(), &mut err);
    assert!(err.is_null());
    let mut err: *mut SimlinError = ptr::null_mut();
    let sim = simlin_sim_new(model, true, &mut err);
    assert!(err.is_null(), "sim_new with enable_ltm=true must succeed");
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null(), "VM run_to_end must succeed");
    let mut err: *mut SimlinError = ptr::null_mut();
    // Raw graph (include_internal=true) so the VM/wasm parity test below
    // compares the same view on both sides; logistic_growth has no macros, so
    // the collapsed view would be identical anyway.
    let links = simlin_analyze_get_links(sim, true, &mut err);
    assert!(err.is_null(), "VM analyze_get_links must succeed");
    assert!(!links.is_null());
    simlin_sim_unref(sim);
    simlin_model_unref(model);
    links
}

/// One link snapshotted out of a `*mut SimlinLinks` so the FFI buffers can be
/// freed before the parity comparison: `(from, to, polarity, score,
/// relative_score)`.
type LinkSnapshot = (String, String, SimlinLinkPolarity, Vec<f64>, Vec<f64>);

/// Convert a `*mut SimlinLinks` into owned `LinkSnapshot`s.
unsafe fn snapshot_links(links: *mut SimlinLinks) -> Vec<LinkSnapshot> {
    let count = (*links).count;
    let slice = if count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts((*links).links, count)
    };
    let mut out = Vec::with_capacity(count);
    for link in slice {
        let from = std::ffi::CStr::from_ptr(link.from)
            .to_str()
            .unwrap()
            .to_string();
        let to = std::ffi::CStr::from_ptr(link.to)
            .to_str()
            .unwrap()
            .to_string();
        let scores = if link.score.is_null() || link.score_len == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(link.score, link.score_len).to_vec()
        };
        let rel_scores = if link.relative_score.is_null() || link.relative_score_len == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(link.relative_score, link.relative_score_len).to_vec()
        };
        out.push((from, to, link.polarity, scores, rel_scores));
    }
    out
}

/// AC2.1: per-link scores returned by `simlin_analyze_links_from_wasm_results`
/// match the VM oracle within a tight numeric tolerance (the wasm/VM parity
/// the engine already enforces on scalar models).
#[test]
fn links_from_wasm_match_vm() {
    unsafe {
        let run = compile_and_run_logistic_growth_ltm();

        let mut err: *mut SimlinError = ptr::null_mut();
        let from_wasm = simlin_analyze_links_from_wasm_results(
            run.model,
            run.slab_bytes.as_ptr(),
            run.slab_bytes.len(),
            run.layout_bytes_ptr,
            run.layout_bytes_len,
            true,
            &mut err,
        );
        assert!(
            err.is_null(),
            "simlin_analyze_links_from_wasm_results must succeed"
        );
        assert!(!from_wasm.is_null());
        let wasm_links = snapshot_links(from_wasm);
        simlin_free_links(from_wasm);

        let vm_links_ptr = vm_oracle_links(run.project);
        let vm_links = snapshot_links(vm_links_ptr);
        simlin_free_links(vm_links_ptr);

        // The link *set* (from, to, polarity) must be identical; the VM and
        // wasm both drive the same `analyze_links_core` over the same
        // structure, so any divergence here is a regression.  Map the C-ABI
        // polarity enum to an Ord-able u8 for sorting.
        let polarity_to_u8 = |p: SimlinLinkPolarity| -> u8 {
            match p {
                SimlinLinkPolarity::Unknown => 0,
                SimlinLinkPolarity::Positive => 1,
                SimlinLinkPolarity::Negative => 2,
            }
        };
        let key = |l: &LinkSnapshot| (l.0.clone(), l.1.clone(), polarity_to_u8(l.2));
        let mut wasm_keys: Vec<_> = wasm_links.iter().map(key).collect();
        let mut vm_keys: Vec<_> = vm_links.iter().map(key).collect();
        wasm_keys.sort();
        vm_keys.sort();
        assert_eq!(
            wasm_keys, vm_keys,
            "wasm and VM must produce the same (from, to, polarity) set"
        );

        assert!(
            wasm_keys.iter().any(|(_, _, p)| *p != 0),
            "scalar LTM model must yield at least one non-Unknown polarity"
        );

        // Build (from, to) → (raw, relative) score-series maps for the
        // comparison.  Both series must match the VM backend by construction
        // (the shared `analyze_links_core` -> `attach_relative_scores` runs
        // identically for both backends), so this is the AC5.1 parity guard
        // extended to the GH #652 relative series.
        let mut wasm_map: std::collections::HashMap<(String, String), (Vec<f64>, Vec<f64>)> =
            wasm_links
                .into_iter()
                .map(|(f, t, _, s, r)| ((f, t), (s, r)))
                .collect();
        let vm_map: std::collections::HashMap<(String, String), (Vec<f64>, Vec<f64>)> = vm_links
            .into_iter()
            .map(|(f, t, _, s, r)| ((f, t), (s, r)))
            .collect();

        // Every link with a VM-side score must also have a wasm-side score
        // (raw and relative) of the same length, agreeing to within 1e-6
        // elementwise.  A link with no VM score likewise has no wasm score.
        let mut scored = 0usize;
        let mut rel_scored = 0usize;
        for (k, (vm_scores, vm_rel)) in &vm_map {
            let (wasm_scores, wasm_rel) = wasm_map
                .remove(k)
                .unwrap_or_else(|| panic!("wasm missing scores for link {:?}", k));
            assert_eq!(
                vm_scores.len(),
                wasm_scores.len(),
                "score-series length mismatch for {:?}",
                k
            );
            assert_eq!(
                vm_rel.len(),
                wasm_rel.len(),
                "relative-score-series length mismatch for {:?}",
                k
            );
            // A scored link always has a relative series of the same length.
            assert_eq!(
                vm_scores.len(),
                vm_rel.len(),
                "VM raw/relative length mismatch for {:?}",
                k
            );
            if !vm_scores.is_empty() {
                scored += 1;
                for (i, (v, w)) in vm_scores.iter().zip(wasm_scores.iter()).enumerate() {
                    assert!(
                        (v - w).abs() < 1e-6,
                        "link {:?} score mismatch at step {i}: vm={v} wasm={w}",
                        k
                    );
                }
            }
            if !vm_rel.is_empty() {
                rel_scored += 1;
                for (i, (v, w)) in vm_rel.iter().zip(wasm_rel.iter()).enumerate() {
                    // Relative scores are bounded in [-1, 1]; NaN can appear
                    // only at an Inf-dominated step, treated as matching.
                    if v.is_nan() && w.is_nan() {
                        continue;
                    }
                    assert!(
                        (v - w).abs() < 1e-6,
                        "link {:?} relative-score mismatch at step {i}: vm={v} wasm={w}",
                        k
                    );
                    assert!(
                        v.abs() <= 1.0 + 1e-9,
                        "VM relative score {v} out of [-1,1] for {:?} at step {i}",
                        k
                    );
                }
            }
        }
        assert!(
            scored > 0,
            "expected at least one link with a non-empty score series"
        );
        assert!(
            rel_scored > 0,
            "expected at least one link with a non-empty relative-score series"
        );
        assert!(
            wasm_map.is_empty(),
            "wasm produced extra links the VM did not: {:?}",
            wasm_map.keys().collect::<Vec<_>>()
        );
    }
}

/// Read the rel-loop-score series for `loop_id` from a VM-backed sim through
/// the existing FFI.  Mirrors the helper pattern in `tests/analysis.rs`.
unsafe fn vm_rel_loop_series(sim: *mut SimlinSim, loop_id: &str, n_chunks: usize) -> Vec<f64> {
    let mut scores = vec![0.0_f64; n_chunks];
    let id_c = std::ffi::CString::new(loop_id).unwrap();
    let mut written: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_analyze_get_relative_loop_score(
        sim,
        id_c.as_ptr(),
        scores.as_mut_ptr(),
        scores.len(),
        &mut written,
        &mut err,
    );
    assert!(
        err.is_null(),
        "VM rel-loop-score for '{loop_id}' must succeed"
    );
    scores.truncate(written);
    scores
}

/// Read the rel-loop-score series for `loop_id` from the from-wasm FFI.
unsafe fn from_wasm_rel_loop_series(run: &WasmRun, loop_id: &str, n_chunks: usize) -> Vec<f64> {
    let mut scores = vec![0.0_f64; n_chunks];
    let id_c = std::ffi::CString::new(loop_id).unwrap();
    let mut written: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_analyze_rel_loop_score_from_wasm_results(
        run.model,
        run.slab_bytes.as_ptr(),
        run.slab_bytes.len(),
        run.layout_bytes_ptr,
        run.layout_bytes_len,
        id_c.as_ptr(),
        scores.as_mut_ptr(),
        scores.len(),
        &mut written,
        &mut err,
    );
    assert!(
        err.is_null(),
        "from-wasm rel-loop-score for '{loop_id}' must succeed"
    );
    scores.truncate(written);
    scores
}

/// AC2.2: rel-loop-score series returned by
/// `simlin_analyze_rel_loop_score_from_wasm_results` match the VM oracle to
/// within 1e-6 for every loop id `simlin_analyze_get_loops` enumerates on a
/// scalar LTM model.
///
/// The logistic-growth scalar fixture only exposes scalar (bare-id) loops, so
/// the subscripted-id parity that Phase 4 covers is asserted via the existing
/// VM-rel-loop-score tests in `tests/analysis.rs` (the
/// `test_arrayed_*`/`test_subscripted_*` suite) -- the new from-wasm twin
/// will pick up subscripted-id coverage when Phase 4's arrayed wasm-LTM
/// model lands.  Documenting the deferral here so a reader doesn't read this
/// test's "scalar only" coverage as a gap.
#[test]
fn rel_loop_score_from_wasm_matches_vm() {
    unsafe {
        let run = compile_and_run_logistic_growth_ltm();

        // Enumerate the loop ids by going through the VM FFI's
        // simlin_analyze_get_loops (the loop set is structure-driven, so
        // the choice of FFI here is incidental -- both backends agree).
        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(run.model, &mut err);
        assert!(err.is_null(), "analyze_get_loops must succeed");
        assert!(!loops.is_null());
        let loop_count = (*loops).count;
        assert!(
            loop_count > 0,
            "logistic-growth must expose at least one loop"
        );
        let slice = std::slice::from_raw_parts((*loops).loops, loop_count);
        let loop_ids: Vec<String> = slice
            .iter()
            .map(|l| std::ffi::CStr::from_ptr(l.id).to_str().unwrap().to_string())
            .collect();
        simlin_free_loops(loops);

        // Build a VM-backed sim once, reuse it across all loop ids -- the
        // VM-side cache amortizes the partition denominators across queries
        // (mirrors how `pysimlin`/the TS engine drive the FFI).
        let model_name = std::ffi::CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let vm_model = simlin_project_get_model(run.project, model_name.as_ptr(), &mut err);
        assert!(err.is_null());
        let mut err: *mut SimlinError = ptr::null_mut();
        let vm_sim = simlin_sim_new(vm_model, true, &mut err);
        assert!(err.is_null());
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_to_end(vm_sim, &mut err);
        assert!(err.is_null());
        let mut step_count: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_get_stepcount(vm_sim, &mut step_count, &mut err);
        assert!(err.is_null());

        let mut any_nonzero = false;
        let mut subscripted_seen = false;
        for loop_id in &loop_ids {
            let vm = vm_rel_loop_series(vm_sim, loop_id, step_count);
            let from_wasm = from_wasm_rel_loop_series(&run, loop_id, step_count);
            assert_eq!(
                vm.len(),
                from_wasm.len(),
                "step-count mismatch for loop '{loop_id}'"
            );
            for (i, (v, w)) in vm.iter().zip(from_wasm.iter()).enumerate() {
                assert!(
                    (v - w).abs() < 1e-6,
                    "loop '{loop_id}' rel-score step {i}: vm={v} wasm={w}"
                );
                if v.abs() > 0.0 {
                    any_nonzero = true;
                }
            }
            if loop_id.contains('[') {
                subscripted_seen = true;
            }
        }
        assert!(
            any_nonzero,
            "expected at least one nonzero rel-loop-score sample across all loops"
        );

        // The scalar logistic-growth model has no subscripted loop ids.  When
        // Phase 4 lands an arrayed wasm-LTM fixture, the assertion below will
        // exercise the subscripted-id path in this same test (rather than a
        // separate one); for now, the assertion is a documented deferral, not
        // a coverage gap.
        let _ = subscripted_seen;
    }
}

/// The wasm blob's results region is allocated for the full `n_chunks`
/// capacity, but the live `G_SAVED` counter records how many rows the
/// simulation has actually written.  Hosts are expected to extract only
/// `saved_steps * n_slots * 8` bytes -- not the slab's full capacity -- so
/// the analytic core never sees uninit/stale tail rows on a fresh,
/// just-reset, or partially-run sim.  This test exercises that contract on
/// `simlin_analyze_links_from_wasm_results`: it computes link scores from
/// the full slab as the oracle, then re-computes them from a truncated
/// (first-half) slab and asserts each per-link score series is exactly the
/// first-half of the oracle (elementwise equality -- both runs share the
/// same f64 inputs, so the truncated answer should be bit-identical, not
/// just within tolerance).
#[test]
fn links_from_wasm_truncated_slab_matches_prefix() {
    unsafe {
        let run = compile_and_run_logistic_growth_ltm();
        let layout = parse_layout(std::slice::from_raw_parts(
            run.layout_bytes_ptr,
            run.layout_bytes_len,
        ));
        // Sanity check the fixture: at least 4 saved rows, so truncating to
        // half still leaves a meaningful series for the per-link assertion.
        assert!(
            layout.n_chunks >= 4,
            "fixture must have >=4 saved rows to make truncation meaningful"
        );
        let full_rows = layout.n_chunks;
        let half_rows = full_rows / 2;
        assert!(
            half_rows > 0 && half_rows < full_rows,
            "half_rows={half_rows} must be a strict prefix of full_rows={full_rows}"
        );

        // Oracle: link scores computed from the full saved slab.
        let mut err: *mut SimlinError = ptr::null_mut();
        let full_links = simlin_analyze_links_from_wasm_results(
            run.model,
            run.slab_bytes.as_ptr(),
            run.slab_bytes.len(),
            run.layout_bytes_ptr,
            run.layout_bytes_len,
            true,
            &mut err,
        );
        assert!(err.is_null(), "full-slab links call must succeed");
        let full = snapshot_links(full_links);
        simlin_free_links(full_links);

        // Truncated: only the first `half_rows` rows -- mirrors what a host
        // would marshal mid-run via `runTo` (or for a just-reset sim).
        let half_slab_bytes = &run.slab_bytes[..half_rows * layout.n_slots * 8];
        let mut err: *mut SimlinError = ptr::null_mut();
        let half_links = simlin_analyze_links_from_wasm_results(
            run.model,
            half_slab_bytes.as_ptr(),
            half_slab_bytes.len(),
            run.layout_bytes_ptr,
            run.layout_bytes_len,
            true,
            &mut err,
        );
        assert!(
            err.is_null(),
            "truncated-slab links call must succeed (saved-rows contract)"
        );
        let half = snapshot_links(half_links);
        simlin_free_links(half_links);

        // Same link topology: both runs share the same SourceProject, so
        // structure-driven keys are identical.
        let key = |l: &LinkSnapshot| (l.0.clone(), l.1.clone());
        let mut full_map: std::collections::HashMap<(String, String), Vec<f64>> =
            full.into_iter().map(|l| (key(&l), l.3)).collect();
        for h in &half {
            let k = key(h);
            let full_scores = full_map
                .remove(&k)
                .unwrap_or_else(|| panic!("truncated run produced unknown link {:?}", k));
            // Empty score series means "this edge has no LTM column" (e.g.
            // self-loops); both calls agree on that by construction.
            if full_scores.is_empty() {
                assert!(h.3.is_empty(), "link {:?} should also be unscored", k);
                continue;
            }
            assert_eq!(
                h.3.len(),
                half_rows,
                "truncated link {:?} score length must equal the row count passed in",
                k
            );
            for (i, (full_v, half_v)) in full_scores
                .iter()
                .take(half_rows)
                .zip(h.3.iter())
                .enumerate()
            {
                // Bit-exact: same f64 inputs, same analytic core.
                assert!(
                    (full_v - half_v).abs() == 0.0,
                    "link {:?} score divergence at step {i}: full={full_v} truncated={half_v}",
                    k,
                );
            }
        }
        assert!(
            full_map.is_empty(),
            "full slab produced links the truncated slab did not: {:?}",
            full_map.keys().collect::<Vec<_>>()
        );
    }
}

/// The saved-rows contract is enforced (not silently rounded): a slab whose
/// f64 length is not a multiple of `n_slots`, or that exceeds the blob's
/// `n_chunks * n_slots` capacity, returns a `SimlinError` rather than
/// reconstructing a malformed `Results`.
#[test]
fn links_from_wasm_rejects_invalid_slab_lengths() {
    unsafe {
        let run = compile_and_run_logistic_growth_ltm();
        let layout = parse_layout(std::slice::from_raw_parts(
            run.layout_bytes_ptr,
            run.layout_bytes_len,
        ));

        // Length not a multiple of n_slots: drop a single f64 (8 bytes) so the
        // remaining slab can't be evenly divided into rows.
        let oversize_step_bytes = run.slab_bytes.len() - 8;
        let mut err: *mut SimlinError = ptr::null_mut();
        let p = simlin_analyze_links_from_wasm_results(
            run.model,
            run.slab_bytes.as_ptr(),
            oversize_step_bytes,
            run.layout_bytes_ptr,
            run.layout_bytes_len,
            true,
            &mut err,
        );
        assert!(p.is_null(), "non-multiple slab length must return null");
        assert!(!err.is_null(), "non-multiple slab length must set an error");
        simlin_error_free(err);

        // Length exceeds the blob's capacity: append one extra row worth of
        // bytes (all zeros) to push past `n_chunks * n_slots * 8`.
        let mut overflow = run.slab_bytes.clone();
        overflow.extend(std::iter::repeat_n(0u8, layout.n_slots * 8));
        let mut err: *mut SimlinError = ptr::null_mut();
        let p = simlin_analyze_links_from_wasm_results(
            run.model,
            overflow.as_ptr(),
            overflow.len(),
            run.layout_bytes_ptr,
            run.layout_bytes_len,
            true,
            &mut err,
        );
        assert!(p.is_null(), "over-capacity slab must return null");
        assert!(!err.is_null(), "over-capacity slab must set an error");
        simlin_error_free(err);
    }
}
