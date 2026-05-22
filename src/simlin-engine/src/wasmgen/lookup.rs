// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each public function emits a self-contained wasm helper
// `Function` (instruction sequence) for one graphical-function lookup mode. No
// I/O; the only side effect is in `#[cfg(test)]`, which executes the emitted
// helpers under the DLR-FT interpreter and compares against the VM's lookup
// functions.

//! Graphical-function lookup helper functions for the wasm simulation backend.
//!
//! The bytecode VM resolves a `Lookup` opcode against a `&[(f64, f64)]` table
//! through one of three functions (`vm.rs:3055-3186`): `lookup` (linear
//! interpolation), `lookup_forward` (step up), and `lookup_backward` (step
//! down). This module emits one wasm helper per mode -- `lookup_interp`,
//! `lookup_forward`, `lookup_backward` -- each over a flat
//! `(data_off: i32, count: i32, index: f64) -> f64` interface, where the table
//! lives in linear memory as `count` consecutive f64 LE `(x, y)` knot pairs
//! starting at byte offset `data_off` (so knot `k` is
//! `x = f64.load[data_off + 16*k]`, `y = f64.load[data_off + 16*k + 8]`).
//! `module.rs` lays these regions out (see `build_gf_regions`); the `Lookup`
//! opcode (`lower.rs`) reads `(data_off, count)` from the GF directory and
//! `call`s the mode's helper.
//!
//! ## The three functions are NOT one function
//!
//! They differ in three ways, mirrored here exactly so the backend takes the
//! same branch the VM does:
//! - **edge clamps**: `lookup_interp` clamps *strictly* (`index < x[0]` /
//!   `index > x[n-1]`); `forward`/`backward` clamp *inclusively* (`<=` / `>=`).
//! - **search**: `interp`/`forward` use a *lower-bound* search
//!   (`x[mid] < index`); `backward` uses an *upper-bound* search
//!   (`x[mid] <= index`).
//! - **result**: `interp` either returns `y[low]` exactly (when
//!   `approx_eq(x[low], index)`, via the Phase 2 helper) or linearly
//!   interpolates between knots `low-1` and `low`; `forward` returns `y[low]`;
//!   `backward` returns `y[low-1]` (the last knot with `x <= index`; for
//!   duplicate x-values, the LAST such knot, since the upper-bound search lands
//!   past every equal x).
//!
//! Each helper guards `count == 0` and a NaN `index` by returning NaN, matching
//! the VM's `table.is_empty()` / `index.is_nan()` early returns.

use wasm_encoder::{BlockType, Function, Instruction as Ins, MemArg, ValType};

/// Bytes per knot: an f64 `x` followed by an f64 `y`.
const KNOT_BYTES: i32 = 16;

// Helper local layout. Params 0..2 are `data_off`/`count`/`index`; the i32
// search cursors follow.
const DATA_OFF: u32 = 0; // i32 byte offset of knot 0
const COUNT: u32 = 1; // i32 point count
const INDEX: u32 = 2; // f64 lookup index
const LOW: u32 = 3; // i32 binary-search low
const HIGH: u32 = 4; // i32 binary-search high
const MID: u32 = 5; // i32 binary-search midpoint

/// An 8-byte (f64) memory access with a static byte `offset` on top of the
/// dynamic address already on the stack. The data region is 8-byte aligned (see
/// `module.rs`), so the natural-alignment hint is valid.
fn knot_memarg(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 3, // log2(8): an 8-byte f64 access
        memory_index: 0,
    }
}

/// Push the byte address of knot `k` (the i32 in `k_local`):
/// `data_off + 16*k`. A subsequent `f64.load` with `knot_memarg(0)` reads `x`,
/// `knot_memarg(8)` reads `y`.
fn push_knot_addr(f: &mut Function, k_local: u32) {
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::LocalGet(k_local));
    f.instruction(&Ins::I32Const(KNOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

/// Push `x[k]` for the knot index in `k_local`.
fn push_x(f: &mut Function, k_local: u32) {
    push_knot_addr(f, k_local);
    f.instruction(&Ins::F64Load(knot_memarg(0)));
}

/// Push `y[k]` for the knot index in `k_local`.
fn push_y(f: &mut Function, k_local: u32) {
    push_knot_addr(f, k_local);
    f.instruction(&Ins::F64Load(knot_memarg(8)));
}

/// Push `x[count-1]` (the last knot's x). Computed without a dedicated local by
/// pushing the address `data_off + 16*(count-1)` inline.
fn push_last_x(f: &mut Function) {
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::I32Const(KNOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(knot_memarg(0)));
}

/// Push `y[count-1]` (the last knot's y).
fn push_last_y(f: &mut Function) {
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::I32Const(KNOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(knot_memarg(8)));
}

/// Emit the two early guards every lookup function shares: `count == 0 -> NaN`
/// and `index != index (NaN) -> NaN`. Mirrors the VM's `table.is_empty()` and
/// `index.is_nan()` early returns.
fn emit_empty_and_nan_guards(f: &mut Function) {
    // if count == 0 { return NaN }
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::F64Const(f64::NAN.into()));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if index != index { return NaN }  (the NaN test)
    f.instruction(&Ins::LocalGet(INDEX));
    f.instruction(&Ins::LocalGet(INDEX));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::F64Const(f64::NAN.into()));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
}

/// Emit the binary search over `[LOW, HIGH)` into `LOW`. `mid_cmp_is_lt` selects
/// the predicate: `true` -> lower bound (`x[mid] < index`), `false` -> upper
/// bound (`x[mid] <= index`). On exit `LOW` is the first index whose `x` fails
/// the predicate (the lower/upper bound), exactly matching the VM's
/// `while low < high { mid; if pred { low = mid+1 } else { high = mid } }`.
///
/// `LOW`/`HIGH` must already be initialized (to `0`/`count`).
fn emit_binary_search(f: &mut Function, mid_cmp_is_lt: bool) {
    f.instruction(&Ins::Block(BlockType::Empty)); // $exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $top

    // while-head: if !(low < high) break  (br depth 1 -> $exit)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::LocalGet(HIGH));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // mid = low + (high - low) / 2  (all non-negative; signed div is exact)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::LocalGet(HIGH));
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32DivS);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MID));

    // pred = x[mid] {<, <=} index
    push_x(f, MID);
    f.instruction(&Ins::LocalGet(INDEX));
    if mid_cmp_is_lt {
        f.instruction(&Ins::F64Lt);
    } else {
        f.instruction(&Ins::F64Le);
    }
    f.instruction(&Ins::If(BlockType::Empty));
    // low = mid + 1
    f.instruction(&Ins::LocalGet(MID));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(LOW));
    f.instruction(&Ins::Else);
    // high = mid
    f.instruction(&Ins::LocalGet(MID));
    f.instruction(&Ins::LocalSet(HIGH));
    f.instruction(&Ins::End);

    f.instruction(&Ins::Br(0)); // continue -> $top
    f.instruction(&Ins::End); // end loop
    f.instruction(&Ins::End); // end block
}

/// Initialize `LOW = 0; HIGH = count`.
fn emit_init_search_bounds(f: &mut Function) {
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(LOW));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::LocalSet(HIGH));
}

/// Build the body of `lookup_interp(data_off: i32, count: i32, index: f64)
/// -> f64`, reproducing the VM's `lookup` (`vm.rs:3055-3102`) exactly:
/// empty/NaN -> NaN; **strict** edge clamps (`index < x[0]` -> `y[0]`,
/// `index > x[n-1]` -> `y[n-1]`); lower-bound binary search; then at `i = low`,
/// `approx_eq(x[i], index)` -> `y[i]`, else linear interpolation between knots
/// `i-1` and `i`.
///
/// `approx_eq_idx` is the module function index of the Phase 2 `approx_eq`
/// helper (`lower::HelperFns::approx_eq`); the at-knot exact-hit test `call`s it
/// so the backend matches the VM's `crate::float::approx_eq` branch.
pub(crate) fn emit_lookup_interp(approx_eq_idx: u32) -> Function {
    let mut f = Function::new([(3, ValType::I32)]); // LOW/HIGH/MID

    emit_empty_and_nan_guards(&mut f);

    // if index < x[0] { return y[0] }  (strict)
    f.instruction(&Ins::LocalGet(INDEX));
    push_x_const0(&mut f);
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    push_y_const0(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if index > x[count-1] { return y[count-1] }  (strict)
    f.instruction(&Ins::LocalGet(INDEX));
    push_last_x(&mut f);
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(BlockType::Empty));
    push_last_y(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    emit_init_search_bounds(&mut f);
    emit_binary_search(&mut f, true); // lower bound

    // i = low. if approx_eq(x[i], index) { return y[i] }
    push_x(&mut f, LOW);
    f.instruction(&Ins::LocalGet(INDEX));
    f.instruction(&Ins::Call(approx_eq_idx));
    f.instruction(&Ins::If(BlockType::Empty));
    push_y(&mut f, LOW);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // else linear interp:
    //   slope = (y[i] - y[i-1]) / (x[i] - x[i-1])
    //   result = (index - x[i-1]) * slope + y[i-1]
    // Reuse MID as the i32 holding `i-1` so x[i-1]/y[i-1] reuse push_x/push_y.
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(MID)); // MID = i-1

    // (index - x[i-1])
    f.instruction(&Ins::LocalGet(INDEX));
    push_x(&mut f, MID);
    f.instruction(&Ins::F64Sub);
    // * slope
    push_y(&mut f, LOW);
    push_y(&mut f, MID);
    f.instruction(&Ins::F64Sub); // y[i] - y[i-1]
    push_x(&mut f, LOW);
    push_x(&mut f, MID);
    f.instruction(&Ins::F64Sub); // x[i] - x[i-1]
    f.instruction(&Ins::F64Div); // slope
    f.instruction(&Ins::F64Mul); // (index - x[i-1]) * slope
    // + y[i-1]
    push_y(&mut f, MID);
    f.instruction(&Ins::F64Add);

    f.instruction(&Ins::End);
    f
}

/// Build the body of `lookup_forward(data_off, count, index) -> f64`,
/// reproducing the VM's `lookup_forward` (`vm.rs:3104-3142`): empty/NaN -> NaN;
/// **inclusive** edge clamps (`index <= x[0]` -> `y[0]`, `index >= x[n-1]` ->
/// `y[n-1]`); the same lower-bound binary search; return `y[low]`. No
/// `approx_eq`, no interpolation.
pub(crate) fn emit_lookup_forward() -> Function {
    let mut f = Function::new([(3, ValType::I32)]); // LOW/HIGH/MID

    emit_empty_and_nan_guards(&mut f);

    // if index <= x[0] { return y[0] }  (inclusive)
    f.instruction(&Ins::LocalGet(INDEX));
    push_x_const0(&mut f);
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));
    push_y_const0(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if index >= x[count-1] { return y[count-1] }  (inclusive)
    f.instruction(&Ins::LocalGet(INDEX));
    push_last_x(&mut f);
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Empty));
    push_last_y(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    emit_init_search_bounds(&mut f);
    emit_binary_search(&mut f, true); // lower bound

    // return y[low]
    push_y(&mut f, LOW);

    f.instruction(&Ins::End);
    f
}

/// Build the body of `lookup_backward(data_off, count, index) -> f64`,
/// reproducing the VM's `lookup_backward` (`vm.rs:3144-3186`): empty/NaN ->
/// NaN; **inclusive** edge clamps; an **upper-bound** binary search
/// (`x[mid] <= index`); return `y[low-1]` (the last knot with `x <= index`; for
/// duplicate x-values, the LAST one). No `approx_eq`, no interpolation.
pub(crate) fn emit_lookup_backward() -> Function {
    let mut f = Function::new([(3, ValType::I32)]); // LOW/HIGH/MID

    emit_empty_and_nan_guards(&mut f);

    // if index <= x[0] { return y[0] }  (inclusive)
    f.instruction(&Ins::LocalGet(INDEX));
    push_x_const0(&mut f);
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));
    push_y_const0(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if index >= x[count-1] { return y[count-1] }  (inclusive)
    f.instruction(&Ins::LocalGet(INDEX));
    push_last_x(&mut f);
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Empty));
    push_last_y(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    emit_init_search_bounds(&mut f);
    emit_binary_search(&mut f, false); // upper bound

    // return y[low-1]  (reuse MID as low-1)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(MID));
    push_y(&mut f, MID);

    f.instruction(&Ins::End);
    f
}

/// Push `x[0]` (`f64.load[data_off + 0]`). The knot-0 address is just
/// `data_off`, so no index arithmetic is needed.
fn push_x_const0(f: &mut Function) {
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::F64Load(knot_memarg(0)));
}

/// Push `y[0]` (`f64.load[data_off + 8]`).
fn push_y_const0(f: &mut Function) {
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::F64Load(knot_memarg(8)));
}

#[cfg(test)]
mod tests {
    use super::super::lower::build_helpers;
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function, FunctionSection,
        Instruction, MemorySection, MemoryType, Module, TypeSection, ValType,
    };

    /// Which lookup helper a test module exports as `f`.
    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Interp,
        Forward,
        Backward,
    }

    /// Resolve a [`Mode`] to its helper function index in the assembled table.
    fn helper_index(mode: Mode) -> u32 {
        let h = build_helpers().fns;
        match mode {
            Mode::Interp => h.lookup_interp,
            Mode::Forward => h.lookup_forward,
            Mode::Backward => h.lookup_backward,
        }
    }

    /// The byte offset the test harness writes the table to (one f64 in, so a
    /// non-zero `data_off` is exercised rather than the degenerate 0).
    const TABLE_BASE: u32 = 8;

    /// Build a module containing *every* helper body (so `lookup_interp`'s
    /// `call approx_eq` resolves) plus a thin exported wrapper
    /// `f(data_off: i32, count: i32, index: f64) -> f64` forwarding to the
    /// helper-under-test, and an exported `memory` seeded with `knots` at
    /// [`TABLE_BASE`] via an active data segment. Mirrors `lower.rs`'s
    /// production assembly: helpers occupy function indices `0..N`, the wrapper
    /// follows at `N`.
    fn build_lookup_module(mode: Mode, knots: &[(f64, f64)]) -> Vec<u8> {
        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;
        let target = helper_index(mode);

        let mut module = Module::new();

        // Type 0 is the wrapper `(i32, i32, f64) -> f64`; helper types follow.
        let mut types = TypeSection::new();
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::F64], [ValType::F64]);
        for hf in &helpers.functions {
            types.ty().function(hf.params.clone(), hf.results.clone());
        }
        module.section(&types);

        let mut functions = FunctionSection::new();
        for (i, _) in helpers.functions.iter().enumerate() {
            functions.function(1 + i as u32);
        }
        functions.function(0);
        module.section(&functions);

        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        let mut exports = ExportSection::new();
        exports.export("f", ExportKind::Func, n_helpers);
        exports.export("memory", ExportKind::Memory, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in &helpers.functions {
            code.function(&hf.body);
        }
        // wrapper: forward (data_off, count, index) to the helper-under-test.
        let mut wrapper = Function::new([]);
        wrapper.instruction(&Instruction::LocalGet(0));
        wrapper.instruction(&Instruction::LocalGet(1));
        wrapper.instruction(&Instruction::LocalGet(2));
        wrapper.instruction(&Instruction::Call(target));
        wrapper.instruction(&Instruction::End);
        code.function(&wrapper);
        module.section(&code);

        // Seed the table at TABLE_BASE as interleaved f64 LE x,y pairs.
        let mut bytes: Vec<u8> = Vec::with_capacity(knots.len() * 16);
        for &(x, y) in knots {
            bytes.extend_from_slice(&x.to_le_bytes());
            bytes.extend_from_slice(&y.to_le_bytes());
        }
        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(TABLE_BASE as i32), bytes);
        module.section(&data);

        module.finish()
    }

    /// Run the emitted lookup helper for `mode` over `knots` at `index` under
    /// the DLR-FT interpreter. The module is (re)built per call; the tables are
    /// tiny (a handful of knots) so this stays well under the per-test budget.
    fn run_lookup(mode: Mode, knots: &[(f64, f64)], index: f64) -> f64 {
        let bytes = build_lookup_module(mode, knots);
        let info = validate(&bytes).expect("lookup module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("lookup module must instantiate")
            .module_addr;
        let f = store
            .instance_export(module, "f")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(i32, i32, f64), f64>(
                f,
                (TABLE_BASE as i32, knots.len() as i32, index),
            )
            .expect("invocation must succeed")
    }

    /// The VM oracle for `mode` (the exact function the helper reproduces).
    fn vm_lookup(mode: Mode, knots: &[(f64, f64)], index: f64) -> f64 {
        match mode {
            Mode::Interp => crate::vm::lookup(knots, index),
            Mode::Forward => crate::vm::lookup_forward(knots, index),
            Mode::Backward => crate::vm::lookup_backward(knots, index),
        }
    }

    /// Assert the emitted helper agrees bit-for-bit with the VM oracle at
    /// `index` (NaN compares as NaN). The interp helper routes its at-knot test
    /// through the same `approx_eq` the VM uses, and neither helper does any
    /// transcendental math, so equality is exact -- not within a tolerance.
    fn assert_matches_vm(mode: Mode, knots: &[(f64, f64)], index: f64) {
        let got = run_lookup(mode, knots, index);
        let want = vm_lookup(mode, knots, index);
        if want.is_nan() {
            assert!(
                got.is_nan(),
                "{mode:?} lookup at index {index}: expected NaN, got {got}"
            );
        } else {
            assert_eq!(
                got, want,
                "{mode:?} lookup at index {index}: got {got}, want {want}"
            );
        }
    }

    /// A monotonic-x table with non-uniform spacing and a non-monotone y, so
    /// interpolation, forward, and backward all give distinguishable results.
    const TABLE: &[(f64, f64)] = &[
        (0.0, 10.0),
        (1.0, 20.0),
        (2.5, 5.0),
        (4.0, 40.0),
        (10.0, 0.0),
    ];

    /// A representative set of probe indices spanning every regime: below
    /// range, exactly on each knot, strictly between each pair of knots, and
    /// above range. Shared by all three modes (each mode's oracle defines the
    /// right answer).
    fn probe_indices(knots: &[(f64, f64)]) -> Vec<f64> {
        let mut idx = vec![knots[0].0 - 5.0, knots[knots.len() - 1].0 + 5.0];
        for w in knots.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            idx.push(a); // on a knot
            idx.push((a + b) / 2.0); // strictly between
            // a point near but not on the knot, to exercise the approx_eq edge
            idx.push(a + (b - a) * 1e-3);
        }
        idx.push(knots[knots.len() - 1].0); // the final knot
        idx
    }

    #[test]
    fn lookup_interp_matches_vm_over_domain() {
        for &index in &probe_indices(TABLE) {
            assert_matches_vm(Mode::Interp, TABLE, index);
        }
    }

    #[test]
    fn lookup_forward_matches_vm_over_domain() {
        for &index in &probe_indices(TABLE) {
            assert_matches_vm(Mode::Forward, TABLE, index);
        }
    }

    #[test]
    fn lookup_backward_matches_vm_over_domain() {
        for &index in &probe_indices(TABLE) {
            assert_matches_vm(Mode::Backward, TABLE, index);
        }
    }

    #[test]
    fn lookup_all_modes_below_and_above_range() {
        // The edge clamps differ (interp strict, forward/backward inclusive) but
        // all three return the boundary y for an out-of-range index; assert each
        // against its own oracle so the strict-vs-inclusive distinction is
        // exercised at the boundary itself.
        for mode in [Mode::Interp, Mode::Forward, Mode::Backward] {
            assert_matches_vm(mode, TABLE, -100.0); // below x[0]
            assert_matches_vm(mode, TABLE, 1000.0); // above x[n-1]
            assert_matches_vm(mode, TABLE, TABLE[0].0); // exactly x[0]
            assert_matches_vm(mode, TABLE, TABLE[TABLE.len() - 1].0); // exactly x[n-1]
        }
    }

    #[test]
    fn lookup_single_point_table() {
        // A one-knot table: every index clamps to that knot's y for all modes.
        let single: &[(f64, f64)] = &[(3.0, 7.0)];
        for mode in [Mode::Interp, Mode::Forward, Mode::Backward] {
            for &index in &[-1.0, 3.0, 3.0 - 1e-9, 3.0 + 1e-9, 100.0] {
                assert_matches_vm(mode, single, index);
            }
        }
    }

    #[test]
    fn lookup_backward_duplicate_x_returns_last() {
        // Duplicate x-values: backward must return the y of the LAST knot with
        // that x (the upper-bound search lands past every equal x, then steps
        // back one). The interp/forward modes are also checked for consistency
        // with their own oracle on the same table.
        let dup: &[(f64, f64)] = &[
            (0.0, 0.0),
            (2.0, 10.0),
            (2.0, 20.0),
            (2.0, 30.0),
            (5.0, 50.0),
        ];
        // Exactly on the duplicated x, and just inside either side of it.
        for &index in &[2.0, 1.999, 2.001, 0.0, 5.0, 3.5] {
            assert_matches_vm(Mode::Backward, dup, index);
            assert_matches_vm(Mode::Forward, dup, index);
            assert_matches_vm(Mode::Interp, dup, index);
        }
    }

    #[test]
    fn lookup_nan_index_returns_nan_all_modes() {
        for mode in [Mode::Interp, Mode::Forward, Mode::Backward] {
            assert!(
                run_lookup(mode, TABLE, f64::NAN).is_nan(),
                "{mode:?} lookup of a NaN index must be NaN"
            );
        }
    }

    #[test]
    fn lookup_empty_table_returns_nan_all_modes() {
        // count == 0 -> NaN for every mode (matching the VM's table.is_empty()).
        // The wrapper passes count = 0; data_off is irrelevant (never read).
        for mode in [Mode::Interp, Mode::Forward, Mode::Backward] {
            assert!(
                run_lookup(mode, &[], 1.0).is_nan(),
                "{mode:?} lookup of an empty table must be NaN"
            );
        }
    }

    #[test]
    fn lookup_interp_exact_knot_uses_approx_eq() {
        // The interp helper returns y[i] exactly when approx_eq(x[i], index),
        // matching the VM. A one-ULP-perturbed index at a knot is approx-equal,
        // so it must return that knot's y exactly (NOT an interpolated value).
        // The VM oracle encodes the same approx_eq decision.
        let knot_x = TABLE[2].0; // 2.5
        let perturbed = f64::from_bits(knot_x.to_bits() + 1);
        assert_matches_vm(Mode::Interp, TABLE, perturbed);
        // And the exact knot returns its y exactly.
        let got = run_lookup(Mode::Interp, TABLE, knot_x);
        assert_eq!(got, TABLE[2].1, "interp at the exact knot returns y[i]");
    }
}
