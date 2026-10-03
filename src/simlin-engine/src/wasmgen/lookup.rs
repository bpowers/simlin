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
//! through one function per `LookupMode` (`vm::lookup_in_mode`): `lookup`
//! (linear interpolation), `lookup_forward` (step up), `lookup_backward` (step
//! down) and `lookup_extrapolate` (linear interpolation, the end segments
//! extended). This module emits one wasm helper per mode -- `lookup_interp`,
//! `lookup_forward`, `lookup_backward`, `lookup_extrapolate` -- each over a flat
//! `(data_off: i32, count: i32, index: f64) -> f64` interface, where the table
//! lives in linear memory as `count` consecutive f64 LE `(x, y)` knot pairs
//! starting at byte offset `data_off` (so knot `k` is
//! `x = f64.load[data_off + 16*k]`, `y = f64.load[data_off + 16*k + 8]`).
//! `module.rs` lays these regions out (see `build_gf_regions`); the `Lookup`
//! opcode (`lower.rs`) reads `(data_off, count)` from the GF directory and
//! `call`s the mode's helper.
//!
//! ## The functions are NOT one function
//!
//! `extrapolate` is `interp` inside the table and a line beyond it. The other
//! three differ in three ways, mirrored here exactly so the backend takes the
//! same branch the VM does:
//! - **edge clamps**: `lookup_interp` clamps *strictly* (`index < x[0]` /
//!   `index > x[n-1]`); `forward` clamps `index <= x[0]` and
//!   `index > x[n-1]`, `backward` `index < x[0]` and `index >= x[n-1]`: each
//!   is inclusive at the end whose knot is the answer by the mode's own rule,
//!   and searches at the other, so points sharing an x at an end are read as
//!   they are anywhere else.
//! - **search**: `interp`/`forward` use a *lower-bound* search
//!   (`x[mid] < index`); `backward` uses an *upper-bound* search
//!   (`x[mid] <= index`).
//! - **result**: `interp` either returns `y[low]` exactly (when `low == 0` or
//!   `approx_eq(x[low], index)`, via the Phase 2 helper) or linearly
//!   interpolates between knots `low-1` and `low`; `forward` returns `y[low]`;
//!   `backward` returns `y[low-1]` (the last knot with `x <= index`; for
//!   duplicate x-values, the LAST such knot, since the upper-bound search lands
//!   past every equal x), or `y[0]` when `low == 0`.
//!
//! No helper reads before the table. `low == 0` past the edge clamps means an
//! x no comparison could order (a NaN, which `parse_table` refuses), and both
//! the VM and these helpers answer with the first knot.
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
/// -> f64`, reproducing the VM's `vm::lookup` exactly:
/// empty/NaN -> NaN; **strict** edge clamps (`index < x[0]` -> `y[0]`,
/// `index > x[n-1]` -> `y[n-1]`); lower-bound binary search; then at `i = low`,
/// `i == 0` or `approx_eq(x[i], index)` -> `y[i]`, else linear interpolation
/// between knots `i-1` and `i`.
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

    // i = low. if i == 0 { return y[0] }  (no knot before it)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    push_y_const0(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if approx_eq(x[i], index) { return y[i] }
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
/// reproducing the VM's `lookup_forward`: empty/NaN -> NaN; `index <= x[0]` ->
/// `y[0]` (inclusive: the first knot is the first at or above it) and
/// `index > x[n-1]` -> `y[n-1]` (strict: an index at the last x is searched
/// for); the same lower-bound binary search; return `y[low]`. No `approx_eq`,
/// no interpolation.
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

    // return y[low]
    push_y(&mut f, LOW);

    f.instruction(&Ins::End);
    f
}

/// Build the body of `lookup_backward(data_off, count, index) -> f64`,
/// reproducing the VM's `lookup_backward`: empty/NaN -> NaN; `index < x[0]` ->
/// `y[0]` (strict: an index at the first x is searched for) and
/// `index >= x[n-1]` -> `y[n-1]` (inclusive: the last knot is the last at or
/// below it); an **upper-bound** binary search
/// (`x[mid] <= index`); return `y[low-1]` (the last knot with `x <= index`; for
/// duplicate x-values, the LAST one), or `y[0]` when `low == 0`. No
/// `approx_eq`, no interpolation.
pub(crate) fn emit_lookup_backward() -> Function {
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

    // if low == 0 { return y[0] }  (no knot before it)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    push_y_const0(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // return y[low-1]  (reuse MID as low-1)
    f.instruction(&Ins::LocalGet(LOW));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(MID));
    push_y(&mut f, MID);

    f.instruction(&Ins::End);
    f
}

/// The f64 working local of [`emit_lookup_extrapolate`]: the x distance
/// between an end knot and its neighbor.
const DX: u32 = 6;

/// Build the body of `lookup_extrapolate(data_off, count, index) -> f64`,
/// reproducing the VM's `lookup_extrapolate`: with two or more knots, an index
/// strictly below the first x or above the last is answered by the line
/// through the two knots at that end; anything else (an index inside the
/// table, a table of fewer than two knots, a NaN index, an empty table) is
/// `lookup_interp`'s answer, which this `call`s through `interp_idx`.
pub(crate) fn emit_lookup_extrapolate(interp_idx: u32) -> Function {
    // LOW/HIGH/MID, then DX. LOW holds the end knot's index and MID its
    // neighbor's while a line is extended.
    let mut f = Function::new([(3, ValType::I32), (1, ValType::F64)]);

    // if count >= 2
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::If(BlockType::Empty));

    // if index < x[0] { return extend(knot 0, knot 1) }
    f.instruction(&Ins::LocalGet(INDEX));
    push_x_const0(&mut f);
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(LOW));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::LocalSet(MID));
    emit_extend_end_segment(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if index > x[count-1] { return extend(knot count-1, knot count-2) }
    f.instruction(&Ins::LocalGet(INDEX));
    push_last_x(&mut f);
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(LOW));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(MID));
    emit_extend_end_segment(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::End); // count >= 2

    // return lookup_interp(data_off, count, index)
    f.instruction(&Ins::LocalGet(DATA_OFF));
    f.instruction(&Ins::LocalGet(COUNT));
    f.instruction(&Ins::LocalGet(INDEX));
    f.instruction(&Ins::Call(interp_idx));

    f.instruction(&Ins::End);
    f
}

/// Push the VM's `extend_end_segment(end, inner, index)` for the end knot in
/// `LOW` and its neighbor in `MID`: `y[end]` when the two share an x or a y,
/// else `y[end] + (index - x[end]) * ((y[end] - y[inner]) / dx)`, the
/// operations in the VM's order so the two agree bit for bit.
fn emit_extend_end_segment(f: &mut Function) {
    // dx = x[end] - x[inner]; dx == 0 || y[end] - y[inner] == 0
    push_x(f, LOW);
    push_x(f, MID);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalTee(DX));
    f.instruction(&Ins::F64Const(0.0.into()));
    f.instruction(&Ins::F64Eq);
    push_y(f, LOW);
    push_y(f, MID);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Const(0.0.into()));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    push_y(f, LOW);
    f.instruction(&Ins::Else);
    push_y(f, LOW);
    f.instruction(&Ins::LocalGet(INDEX));
    push_x(f, LOW);
    f.instruction(&Ins::F64Sub); // index - x[end]
    push_y(f, LOW);
    push_y(f, MID);
    f.instruction(&Ins::F64Sub); // y[end] - y[inner]
    f.instruction(&Ins::LocalGet(DX));
    f.instruction(&Ins::F64Div); // slope
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::End);
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
    use crate::bytecode::LookupMode;
    use crate::vm::lookup_in_mode;
    use checked::Store;
    use proptest::prelude::*;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, FunctionSection,
        MemorySection, MemoryType, Module, TypeSection,
    };

    /// The byte offset the harness writes the table to. The memory before and
    /// after the table holds [`POISON`], a value no test table holds, so a
    /// helper that reads outside its table answers with it (or with arithmetic
    /// on it) and disagrees with the VM.
    const TABLE_BASE: u32 = 64;
    const POISON: f64 = -987654321.25;

    /// Each mode's helper is exported under its index in [`LookupMode::ALL`].
    fn export_name(mode: LookupMode) -> String {
        format!("mode{}", mode as u8)
    }

    /// A module holding every helper body (so the calls between helpers
    /// resolve), each mode's lookup helper exported, and a memory seeded with
    /// `knots` at [`TABLE_BASE`]. Helpers occupy function indices `0..N`, as in
    /// `lower.rs`'s production assembly.
    fn build_lookup_module(knots: &[(f64, f64)]) -> Vec<u8> {
        let helpers = build_helpers();
        let mut module = Module::new();

        let mut types = TypeSection::new();
        for hf in &helpers.functions {
            types.ty().function(hf.params.clone(), hf.results.clone());
        }
        module.section(&types);

        let mut functions = FunctionSection::new();
        for (i, _) in helpers.functions.iter().enumerate() {
            functions.function(i as u32);
        }
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
        for mode in LookupMode::ALL {
            exports.export(
                &export_name(mode),
                ExportKind::Func,
                helpers.fns.lookup(mode),
            );
        }
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in &helpers.functions {
            code.function(&hf.body);
        }
        module.section(&code);

        let mut bytes: Vec<u8> = Vec::new();
        for _ in 0..(TABLE_BASE / 8) {
            bytes.extend_from_slice(&POISON.to_le_bytes());
        }
        for &(x, y) in knots {
            bytes.extend_from_slice(&x.to_le_bytes());
            bytes.extend_from_slice(&y.to_le_bytes());
        }
        for _ in 0..4 {
            bytes.extend_from_slice(&POISON.to_le_bytes());
        }
        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(0), bytes);
        module.section(&data);

        module.finish()
    }

    /// Every mode's emitted helper over `knots`, run at each of `indexes`
    /// under the DLR-FT interpreter: `answers[i][m]` is mode `ALL[m]` at
    /// `indexes[i]`. One module serves the whole table.
    fn run_lookups(knots: &[(f64, f64)], indexes: &[f64]) -> Vec<[f64; LookupMode::ALL.len()]> {
        let bytes = build_lookup_module(knots);
        let info = validate(&bytes).expect("lookup module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("lookup module must instantiate")
            .module_addr;
        let helpers = LookupMode::ALL.map(|mode| {
            store
                .instance_export(module, &export_name(mode))
                .unwrap()
                .as_func()
                .unwrap()
        });
        indexes
            .iter()
            .map(|&index| {
                helpers.map(|f| {
                    store
                        .invoke_simple_typed::<(i32, i32, f64), f64>(
                            f,
                            (TABLE_BASE as i32, knots.len() as i32, index),
                        )
                        .expect("invocation must succeed")
                })
            })
            .collect()
    }

    fn run_lookup(mode: LookupMode, knots: &[(f64, f64)], index: f64) -> f64 {
        let at = LookupMode::ALL.iter().position(|m| *m == mode).unwrap();
        run_lookups(knots, &[index])[0][at]
    }

    /// Every mode's helper agrees bit for bit with the VM's function for that
    /// mode over `knots` at each index (a NaN is a NaN): none of them does
    /// transcendental math, and each runs the VM's operations in the VM's
    /// order, so the agreement is exact and not within a tolerance.
    fn check_matches_vm(knots: &[(f64, f64)], indexes: &[f64]) -> Result<(), String> {
        let answers = run_lookups(knots, indexes);
        for (index, answers) in indexes.iter().zip(answers) {
            for (mode, got) in LookupMode::ALL.into_iter().zip(answers) {
                let want = lookup_in_mode(mode, knots, *index);
                if !(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan())) {
                    return Err(format!(
                        "{mode:?} over {knots:?} at {index}: wasm {got}, vm {want}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn assert_matches_vm(knots: &[(f64, f64)], indexes: &[f64]) {
        if let Err(why) = check_matches_vm(knots, indexes) {
            panic!("{why}");
        }
    }

    /// A monotonic-x table with non-uniform spacing and a non-monotone y, so
    /// every mode gives a distinguishable result.
    const TABLE: &[(f64, f64)] = &[
        (0.0, 10.0),
        (1.0, 20.0),
        (2.5, 5.0),
        (4.0, 40.0),
        (10.0, 0.0),
    ];

    /// Indexes spanning every regime of `knots`: below range, on each knot,
    /// strictly between each pair, just past each knot (the `approx_eq` edge),
    /// and above range.
    fn sample_indices(knots: &[(f64, f64)]) -> Vec<f64> {
        let mut idx = vec![knots[0].0 - 5.0, knots[knots.len() - 1].0 + 5.0];
        for w in knots.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            idx.push(a);
            idx.push((a + b) / 2.0);
            idx.push(a + (b - a) * 1e-3);
        }
        idx.push(knots[knots.len() - 1].0);
        idx
    }

    #[test]
    fn every_mode_matches_the_vm_over_a_tables_domain() {
        assert_matches_vm(TABLE, &sample_indices(TABLE));
    }

    #[test]
    fn a_one_point_table_matches_the_vm() {
        assert_matches_vm(&[(3.0, 7.0)], &[-1.0, 3.0, 3.0 - 1e-9, 3.0 + 1e-9, 100.0]);
    }

    /// A NaN first x defeats every comparison against it, so a search can end
    /// at knot 0. Every mode answers with the first knot rather than reading
    /// the bytes before the table.
    #[test]
    fn no_mode_reads_before_a_table_whose_first_x_is_not_a_number() {
        let tables: [&[(f64, f64)]; 2] = [&[(f64::NAN, 7.0)], &[(f64::NAN, 7.0), (1.0, 9.0)]];
        for knots in tables {
            let indexes = [-1.0, 0.0, 0.5];
            assert_matches_vm(knots, &indexes);
            for answers in run_lookups(knots, &indexes) {
                assert_eq!(answers, [7.0; LookupMode::ALL.len()], "{knots:?}");
            }
        }
    }

    /// Repeated x values: stepping back answers with the LAST knot of that x
    /// (the upper-bound search lands past every equal x, then steps back one).
    #[test]
    fn a_repeated_x_matches_the_vm() {
        let dup: &[(f64, f64)] = &[
            (0.0, 0.0),
            (2.0, 10.0),
            (2.0, 20.0),
            (2.0, 30.0),
            (5.0, 50.0),
        ];
        assert_matches_vm(dup, &[2.0, 1.999, 2.001, 0.0, 5.0, 3.5]);
        assert_eq!(run_lookup(LookupMode::Backward, dup, 2.0), 30.0);
    }

    /// Two points that share an x are a vertical step, and each mode reads it
    /// by the rule it reads every knot by, wherever the step sits -- inside
    /// the table or at either end (`LookupMode`'s rustdoc states the rule and
    /// that it is the engine's own).
    #[test]
    fn a_vertical_step_is_read_by_each_modes_rule_wherever_it_sits() {
        /// What `mode` answers at the x of a step from 2 up to 5.
        fn at_the_step(mode: LookupMode) -> f64 {
            match mode {
                // The first listed point: the step takes effect just past x.
                LookupMode::Interpolate | LookupMode::Extrapolate => 2.0,
                // The first point at or above the index.
                LookupMode::Forward => 2.0,
                // The last point at or below the index.
                LookupMode::Backward => 5.0,
            }
        }
        let inside: &[(f64, f64)] = &[(0.0, 1.0), (1.0, 2.0), (1.0, 5.0), (2.0, 6.0)];
        let at_the_start: &[(f64, f64)] = &[(1.0, 2.0), (1.0, 5.0), (2.0, 6.0)];
        let at_the_end: &[(f64, f64)] = &[(0.0, 1.0), (1.0, 2.0), (1.0, 5.0)];
        for knots in [inside, at_the_start, at_the_end] {
            assert_matches_vm(knots, &[-3.0, 0.5, 1.0, 1.5, 9.0]);
            for mode in LookupMode::ALL {
                assert_eq!(
                    run_lookup(mode, knots, 1.0),
                    at_the_step(mode),
                    "{mode:?} at the step of {knots:?}"
                );
            }
        }
        // Beyond a step at an end, every mode answers with that end's outer
        // point: there is no line through a vertical step to extend.
        for mode in LookupMode::ALL {
            assert_eq!(run_lookup(mode, at_the_start, 0.0), 2.0, "{mode:?}");
            assert_eq!(run_lookup(mode, at_the_end, 3.0), 5.0, "{mode:?}");
        }
    }

    #[test]
    fn a_nan_index_and_an_empty_table_are_nan_in_every_mode() {
        for answers in run_lookups(TABLE, &[f64::NAN]) {
            assert!(answers.iter().all(|v| v.is_nan()), "{answers:?}");
        }
        for answers in run_lookups(&[], &[1.0]) {
            assert!(answers.iter().all(|v| v.is_nan()), "{answers:?}");
        }
    }

    /// Interpolation answers `y[i]` exactly when `approx_eq(x[i], index)`, as
    /// the VM does: an index one ULP short of a knot is that knot, not a point
    /// interpolated toward it.
    #[test]
    fn interpolation_at_a_knot_uses_approx_eq() {
        let knot_x = TABLE[2].0;
        let just_below = f64::from_bits(knot_x.to_bits() - 1);
        let just_above = f64::from_bits(knot_x.to_bits() + 1);
        assert_matches_vm(TABLE, &[just_below, knot_x, just_above]);
        assert_eq!(
            run_lookup(LookupMode::Interpolate, TABLE, just_below),
            TABLE[2].1
        );
    }

    /// The example on Vensim's reference pages for the three lookup functions
    /// (vensim.com/documentation/fn_lookup_extrapolate.html,
    /// fn_lookup_forward.html, fn_lookup_backward.html), each over
    /// `LOOK((0,1),(1,1),(2,2))` at -1, 1.5 and 2.5.
    #[test]
    fn each_mode_answers_vensims_documented_example() {
        let look: &[(f64, f64)] = &[(0.0, 1.0), (1.0, 1.0), (2.0, 2.0)];
        let documented = |mode: LookupMode| match mode {
            // A plain lookup: interpolated inside, the end values outside.
            LookupMode::Interpolate => [1.0, 1.5, 2.0],
            LookupMode::Forward => [1.0, 2.0, 2.0],
            LookupMode::Backward => [1.0, 1.0, 2.0],
            LookupMode::Extrapolate => [1.0, 1.5, 2.5],
        };
        let indexes = [-1.0, 1.5, 2.5];
        for mode in LookupMode::ALL {
            for (index, want) in indexes.iter().zip(documented(mode)) {
                assert_eq!(
                    lookup_in_mode(mode, look, *index),
                    want,
                    "vm {mode:?} at {index}"
                );
                assert_eq!(
                    run_lookup(mode, look, *index),
                    want,
                    "wasm {mode:?} at {index}"
                );
            }
        }
    }

    /// Extrapolation extends the line through the two points at an end, and an
    /// end with no line to extend (one point, or two points sharing an x)
    /// answers with its end point. A flat end segment extends as its y to any
    /// index, an infinite one included.
    #[test]
    fn extrapolation_extends_each_end_segment() {
        let rising: &[(f64, f64)] = &[(0.0, 0.0), (1.0, 10.0), (2.0, 10.0)];
        let stepped: &[(f64, f64)] = &[(0.0, 5.0), (0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (2.0, 9.0)];
        let lone: &[(f64, f64)] = &[(3.0, 7.0)];
        let rows = [
            (rising, -1.0, -10.0),
            (rising, 3.0, 10.0),
            (rising, 0.5, 5.0),
            (stepped, -4.0, 5.0),
            (stepped, 7.0, 9.0),
            (lone, 100.0, 7.0),
            (rising, f64::INFINITY, 10.0),
            (rising, f64::NEG_INFINITY, f64::NEG_INFINITY),
            (stepped, f64::INFINITY, 9.0),
            (stepped, f64::NEG_INFINITY, 5.0),
        ];
        for (knots, index, want) in rows {
            assert_eq!(
                lookup_in_mode(LookupMode::Extrapolate, knots, index),
                want,
                "vm over {knots:?} at {index}"
            );
            assert_eq!(
                run_lookup(LookupMode::Extrapolate, knots, index),
                want,
                "wasm over {knots:?} at {index}"
            );
        }
    }

    /// A number a table or an index can hold: small halves, so knots repeat,
    /// fall out of order and are hit exactly; one ULP off them; a signed zero;
    /// the infinities; a NaN.
    fn number() -> impl Strategy<Value = f64> {
        prop_oneof![
            8 => (-4i32..8).prop_map(|n| n as f64 / 2.0),
            1 => (-4i32..8).prop_map(|n| f64::from_bits((n as f64 / 2.0).to_bits().wrapping_add(1))),
            1 => Just(-0.0),
            1 => Just(f64::INFINITY),
            1 => Just(f64::NEG_INFINITY),
            1 => Just(f64::NAN),
        ]
    }

    fn table_and_indexes() -> impl Strategy<Value = (Vec<(f64, f64)>, Vec<f64>)> {
        (
            prop::collection::vec((number(), number()), 0..6),
            prop::collection::vec(number(), 1..6),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// Any table at all -- empty, one point, x out of order, repeated, not
        /// a number or infinite -- is read alike by the VM and the wasm
        /// helpers in every mode, at any index.
        #[test]
        fn every_mode_matches_the_vm_on_any_table((knots, indexes) in table_and_indexes()) {
            prop_assert_eq!(check_matches_vm(&knots, &indexes), Ok(()));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4000))]

        #[test]
        #[ignore = "4000 generated tables through the wasm interpreter; run under the gates profile"]
        fn every_mode_matches_the_vm_on_many_tables((knots, indexes) in table_and_indexes()) {
            prop_assert_eq!(check_matches_vm(&knots, &indexes), Ok(()));
        }
    }
}
