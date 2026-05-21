// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: bytecode + layout data in, wasm `Function` bodies /
// instruction sequences out. No I/O; the only side effect is in `#[cfg(test)]`,
// which executes the emitted modules under the DLR-FT interpreter.

//! Lowering of the bytecode VM's scalar-core opcode set to WebAssembly.
//!
//! The runtime data model mirrors the bytecode VM (`crate::vm`): all variable
//! values live in one flat f64 "slab" in linear memory, addressed by slot
//! offset. A model runs over two chunks at a time -- `curr` (the values at the
//! current timestep) and `next` (the values being computed for the following
//! timestep). `LoadVar` reads from `curr`; `AssignCurr`/`AssignNext` store into
//! `curr`/`next`.
//!
//! Each `Opcode` lowers to a short, mostly 1:1 wasm instruction sequence over
//! the wasm operand stack, reproducing the matching arm of `eval_bytecode`
//! (`vm.rs:1257+`).
//!
//! Three compound assignment opcodes beyond the bare scalar set reach a
//! `CompiledSimulation` consumer, and they all lower here:
//! - `AssignConstCurr` arrives by *two* routes: `compiler::codegen` emits it
//!   directly for any constant-RHS `AssignCurr` (`codegen.rs:1164`), and the
//!   **peephole** pass also fuses a `LoadConstant; AssignCurr` pair into it
//!   (`bytecode.rs:1830`). Either way it rides through the symbolic layer into
//!   `CompiledSimulation`; every model with a constant initial/aux carries it.
//! - `BinOpAssignCurr` / `BinOpAssignNext` are *only* peephole output
//!   (`bytecode.rs:1837`/`1841`, fusing `Op2; Assign{Curr,Next}`). The peephole
//!   pass (`ByteCode::peephole_optimize`, run inside
//!   `Module::compile`/`ByteCodeBuilder::finish`) runs per-variable-fragment in
//!   the incremental pipeline *before* symbolization, so these ride through
//!   too. Every scalar Euler stock integration (`stock + delta`) is one, so
//!   they are part of the scalar core.
//!
//! The late **3-address** pass (`ByteCode::fuse_three_address`) instead runs
//! only on the VM's private execution copy (`vm.rs:395-398`), so its
//! `BinVarVar` / `AssignAddVarVarCurr` / ... family never reaches a consumer.
//!
//! Anything outside the supported scalar core -- an array/module/lookup opcode
//! or a late-fusion superinstruction that somehow appeared -- returns
//! `WasmGenError::Unsupported` rather than emitting a wrong module. (Every
//! `Op2` variant, including `Mod`/`Exp`, is supported as of Phase 2.)
//!
//! ## Emitted helper functions
//!
//! Equality and truthiness route through a single emitted wasm helper,
//! `approx_eq(a: f64, b: f64) -> i32`, that reproduces `crate::float::approx_eq`
//! (`float_cmp` 0.10 defaults) bit-faithfully, so the backend takes the same
//! branch the VM does. Helper functions are assembled into the module ahead of
//! the per-program functions ([`build_helpers`] returns their bodies and a
//! [`HelperFns`] index registry); `module.rs` places them at function indices
//! `0..N` and the per-program + `run` functions at `N..`. `emit_bytecode`
//! references a helper by its stable index (held in [`EmitCtx::helpers`]) via a
//! `call`. Subcomponent B (the transcendental + `pulse` helpers) and later
//! phases extend this by adding a field to [`HelperFns`] and pushing the
//! corresponding body in [`build_helpers`]; no helper index is hard-coded
//! elsewhere, so the per-program offset adjusts automatically.

use wasm_encoder::{Function, Instruction, MemArg, ValType};

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, GraphicalFunctionId, LookupMode, Op2, Opcode,
};
use crate::vm::StepPart;

use super::WasmGenError;
use super::views::ElementAddr;
use super::views::{ViewBase, ViewDesc};

/// Bytes per f64 slot.
const SLOT_SIZE: u32 = 8;
/// Alignment exponent for an 8-byte f64 access (log2(8)).
const F64_ALIGN: u32 = 3;
/// Bytes per GF directory entry (two i32: data byte offset + point count). Must
/// match `module.rs`'s `GF_DIRECTORY_ENTRY_BYTES`, the layout the `Lookup`
/// opcode reads.
const GF_DIRECTORY_ENTRY_BYTES: i32 = 8;

/// Compile-time context for lowering a scalar opcode program over the f64 slab.
///
/// `curr_base`/`next_base` are byte offsets of slot 0 of each chunk within the
/// linear memory. `module_off_local` is the wasm local index holding this
/// instance's `module_off` (the slot base of the module instance within a
/// chunk); the per-program functions take it as their single `i32` parameter.
/// In Phase 1 the root is the only module so `module_off` is always 0, but
/// emitting with it from the start avoids a Phase 7 rewrite.
pub(crate) struct EmitCtx<'a> {
    pub curr_base: u32,
    pub next_base: u32,
    /// Byte offset of the GF directory region (8 bytes/entry, indexed by global
    /// table index: `(data_byte_offset: i32, n_points: i32)`). The `Lookup`
    /// opcode reads `directory_base + table_idx*8` to map a table index to its
    /// data location. Both bases are run-invariant: every per-program function
    /// reads the same read-only GF regions.
    pub gf_directory_base: u32,
    /// Byte offset of the GF data region (every table's `(x,y)` knots as f64 LE
    /// pairs, concatenated). Retained for completeness/Phase-7 reuse; the
    /// per-table absolute data offset the `Lookup` opcode passes to a helper is
    /// read from the directory, so opcode lowering does not consult this field.
    #[allow(dead_code)]
    pub gf_data_base: u32,
    /// Byte offset of slot 0 of the `initial_values` snapshot region (n_slots
    /// wide). `LoadInitial` reads `initial_values[module_off + off]` when the
    /// program being emitted is *not* the initials program. Mirrors the VM's
    /// `initial_values` buffer (`vm.rs:617`).
    pub initial_values_base: u32,
    /// Byte offset of slot 0 of the `prev_values` snapshot region (n_slots
    /// wide). `LoadPrev` reads `prev_values[module_off + off]` once the snapshot
    /// has been taken. Mirrors the VM's `prev_values` buffer.
    pub prev_values_base: u32,
    /// Index of the mutable i32 wasm global `use_prev_fallback` (init 1).
    /// `LoadPrev` gates on it: while set, it yields the caller-supplied fallback
    /// rather than reading `prev_values`. The flag -- not a `TIME == start`
    /// comparison -- is the sole gate, because RK stages move `curr[TIME]` to
    /// trial points before the first snapshot is taken (`vm.rs:1314-1327`).
    pub use_prev_fallback_global: u32,
    /// Which opcode program is being lowered. `LoadInitial` resolves its
    /// "during Initials read `curr`, else read `initial_values`" branch
    /// (`vm.rs:1332-1340`) at compile time from this, since the emitter knows
    /// the program statically.
    pub step_part: StepPart,
    // dt/start_time/final_time are the run-invariant time globals that back the
    // seeds `run` writes into the TIME/DT/INITIAL_TIME/FINAL_TIME memory slots.
    // Opcode lowering reads those values from memory via `LoadGlobalVar` (slots
    // 0..4) rather than from these fields -- the XMILE time builtins lower to
    // `LoadGlobalVar`, and the time-driven `Apply` arms (Step/Ramp/Pulse) read
    // TIME/DT from memory -- so the fields stay unused here. They are retained
    // because a later phase may fold them into compile-time constants.
    #[allow(dead_code)]
    pub dt: f64,
    #[allow(dead_code)]
    pub start_time: f64,
    #[allow(dead_code)]
    pub final_time: f64,
    /// wasm local index holding this instance's `module_off` (i32).
    pub module_off_local: u32,
    /// wasm local index of a scratch f64, used by `AssignCurr`/`AssignNext` to
    /// hold the value while the store address is pushed under it.
    pub scratch_local: u32,
    /// wasm local indices reserved for the `SetCond`/`If` condition register.
    /// Used as a stack: `SetCond` writes the top, `If` reads (and pops) it.
    /// Sized to the program's maximum `If` nesting depth (see
    /// [`max_condition_depth`]).
    pub condition_locals: Vec<u32>,
    /// Three dedicated scratch f64 local indices `[a, b, c]` for the `Apply`
    /// opcode, which always pops exactly three operands (codegen pads). They
    /// are distinct from [`scratch_local`](Self::scratch_local) and the
    /// [`condition_locals`](Self::condition_locals) so an `Apply` inside an
    /// `If` arm (sharing the function) cannot clobber the condition register.
    /// Reserved unconditionally by the function builders (3 unused f64 locals
    /// in a non-`Apply` function are free).
    pub apply_locals: [u32; 3],
    /// Function indices of the module's emitted helper functions, so
    /// value-producing opcodes that need the VM's `approx_eq`/transcendental
    /// semantics can `call` them. The same registry is shared by every
    /// per-program function in a module.
    pub helpers: HelperFns,
    /// Byte offset of slot 0 of the `temp_storage` region (`temp_total_size`
    /// f64 wide). The array view machinery addresses temp element `index` of
    /// temp `temp_id` at `temp_storage_base + (temp_offsets[temp_id] + index)*8`,
    /// mirroring the VM's `temp_storage[temp_offsets[temp_id] + index]`
    /// (`vm.rs:584-586`).
    pub temp_storage_base: u32,
    /// First wasm local index reserved for the dynamic-subscript scratch i32
    /// locals (Task 4): the runtime-offset addend and validity flag a
    /// `ViewSubscriptDynamic` / `PushSubscriptIndex` accumulation draws from. The
    /// function's local declarations reserve `count_extra_i32_locals(bc)` i32s
    /// starting here, past the scratch f64 / condition i32s / `Apply` f64s, so
    /// these never overlap [`apply_locals`](Self::apply_locals). A program with
    /// no dynamic subscripts reserves none and this base is unused.
    pub extra_i32_local_base: u32,
    /// The module's `ByteCodeContext`, holding the compile-time array tables the
    /// view opcodes reference by index: `static_views`, `dim_lists`,
    /// `dimensions`, `subdim_relations`, and `temp_offsets`. Run-invariant and
    /// shared by every per-program function.
    pub ctx: &'a ByteCodeContext,
}

// Reserved global slots (absolute, module-independent), mirroring `crate::vm`.
// `Apply` reads `curr[TIME_OFF]` / `curr[DT_OFF]` for the time-driven builtins.
const TIME_OFF: u16 = 0;
const DT_OFF: u16 = 1;

pub(crate) fn memarg(addr: u64) -> MemArg {
    MemArg {
        offset: addr,
        align: F64_ALIGN,
        memory_index: 0,
    }
}

/// `.into()` keeps this robust to whether `wasm-encoder`'s `F64Const` field is
/// a bare `f64` or an `Ieee64` wrapper across versions.
pub(crate) fn f64_const(v: f64) -> Instruction<'static> {
    Instruction::F64Const(v.into())
}

// ============================================================================
// Emitted helper functions
// ============================================================================

/// Function indices of a module's emitted helper functions.
///
/// Helpers occupy the module's first function slots (`0..N`), so their indices
/// are fixed and known before any per-program function is emitted. This is what
/// lets a value-producing opcode in `emit_bytecode` reference a helper by index
/// (`call`). [`build_helpers`] both emits the bodies and assigns these indices,
/// keeping the index assignment and the body emission in one place.
///
/// To add a helper (Subcomponent B's transcendentals + `pulse`, later phases'
/// lookup/array/allocation helpers): add a field here and push its body in
/// [`build_helpers`], assigning the field from the pre-push `functions.len()`.
/// Do not hard-code a helper's index anywhere else.
#[derive(Clone, Copy)]
pub(crate) struct HelperFns {
    /// `approx_eq(a: f64, b: f64) -> i32` (1 = approximately equal, else 0),
    /// reproducing `crate::float::approx_eq` (`float_cmp` 0.10 defaults).
    pub approx_eq: u32,
    /// `mod_euclid(l: f64, r: f64) -> f64`, reproducing `f64::rem_euclid` (the
    /// VM's `Op2::Mod`): a result in `[0, |r|)`. A self-contained helper (rather
    /// than an inline sequence) because the euclidean remainder needs both
    /// operands live across several uses, exceeding the single assign-scratch
    /// local available to `emit_op2`.
    pub mod_euclid: u32,
    /// `pulse(time, dt, volume, first_pulse, interval) -> f64`, reproducing the
    /// VM's `pulse` (`vm.rs:3036`) including its `while` loop. A helper because
    /// of the loop (an inline expansion would need a wasm `loop`/`br_if` in the
    /// middle of `Apply`'s operand handling).
    pub pulse: u32,
    /// Open-coded transcendental helpers (`super::math`), each `(f64) -> f64`
    /// except [`pow`](Self::pow) which is `(f64, f64) -> f64`. The bodies are
    /// emitted in `super::math`; the composed ones (`tan`/`asin`/`acos`/
    /// `log10`/`pow`) `call` the leaf ones by the indices recorded here, so the
    /// leaves are pushed first in [`build_helpers`]. `pow` is consumed by
    /// `Op2::Exp`; the rest by the `Apply` arm.
    pub exp: u32,
    pub ln: u32,
    pub sin: u32,
    pub cos: u32,
    pub tan: u32,
    pub atan: u32,
    pub asin: u32,
    pub acos: u32,
    pub log10: u32,
    pub pow: u32,
    /// Graphical-function lookup helpers (`super::lookup`), each
    /// `(data_off: i32, count: i32, index: f64) -> f64`, reproducing the VM's
    /// `lookup`/`lookup_forward`/`lookup_backward` (`vm.rs:3055-3186`). The
    /// `Lookup` opcode (`emit_bytecode`) reads `(data_off, count)` from the GF
    /// directory and `call`s the mode's helper. [`lookup_interp`](Self::lookup_interp)
    /// `call`s [`approx_eq`](Self::approx_eq) for its at-knot exact-hit test, so
    /// `approx_eq` is pushed before it in [`build_helpers`].
    pub lookup_interp: u32,
    pub lookup_forward: u32,
    pub lookup_backward: u32,
}

/// One emitted helper function: its signature (so the assembler can register a
/// wasm type for it) and its body (the terminating `End` is included).
pub(crate) struct HelperFn {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
    pub body: Function,
}

/// The emitted helper functions plus the [`HelperFns`] index registry that
/// names them. `functions[i]` is the body for function index `i`.
pub(crate) struct BuiltHelpers {
    pub fns: HelperFns,
    pub functions: Vec<HelperFn>,
}

/// Emit every helper function a module needs, assigning each a stable function
/// index starting at 0.
///
/// The returned [`HelperFns`] records the indices; the caller (`module.rs`)
/// places `functions` at module function indices `0..functions.len()` and emits
/// the per-program + `run` functions after them, threading [`BuiltHelpers::fns`]
/// into each [`EmitCtx`].
pub(crate) fn build_helpers() -> BuiltHelpers {
    let mut functions: Vec<HelperFn> = Vec::new();

    // Push a `(f64, ...) -> f64`-shaped helper and return its assigned index.
    // The index is `functions.len()` *before* the push, so it stays valid no
    // matter how many helpers precede it. Used for every transcendental.
    let push_unary = |functions: &mut Vec<HelperFn>, body: Function| -> u32 {
        let idx = functions.len() as u32;
        functions.push(HelperFn {
            params: vec![ValType::F64],
            results: vec![ValType::F64],
            body,
        });
        idx
    };

    let approx_eq = functions.len() as u32;
    functions.push(HelperFn {
        params: vec![ValType::F64, ValType::F64],
        results: vec![ValType::I32],
        body: emit_approx_eq(),
    });

    let mod_euclid = functions.len() as u32;
    functions.push(HelperFn {
        params: vec![ValType::F64, ValType::F64],
        results: vec![ValType::F64],
        body: emit_mod_euclid(),
    });

    let pulse = functions.len() as u32;
    functions.push(HelperFn {
        params: vec![
            ValType::F64,
            ValType::F64,
            ValType::F64,
            ValType::F64,
            ValType::F64,
        ],
        results: vec![ValType::F64],
        body: emit_pulse(),
    });

    // Leaf transcendentals (no inter-helper calls).
    let exp = push_unary(&mut functions, super::math::emit_exp());
    let ln = push_unary(&mut functions, super::math::emit_ln());
    let sin = push_unary(&mut functions, super::math::emit_sin());
    let cos = push_unary(&mut functions, super::math::emit_cos());
    let atan = push_unary(&mut functions, super::math::emit_atan());

    // Composed transcendentals, referencing the leaves by their recorded index.
    let tan = push_unary(&mut functions, super::math::emit_tan(sin, cos));
    let asin = push_unary(&mut functions, super::math::emit_asin(atan));
    let acos = push_unary(&mut functions, super::math::emit_acos(asin));
    let log10 = push_unary(&mut functions, super::math::emit_log10(ln));

    // `pow` is the only binary helper.
    let pow = functions.len() as u32;
    functions.push(HelperFn {
        params: vec![ValType::F64, ValType::F64],
        results: vec![ValType::F64],
        body: super::math::emit_pow(exp, ln),
    });

    // GF lookup helpers, each `(data_off: i32, count: i32, index: f64) -> f64`.
    // `lookup_interp` `call`s `approx_eq` (assigned above), so its body is built
    // with that index.
    let push_lookup = |functions: &mut Vec<HelperFn>, body: Function| -> u32 {
        let idx = functions.len() as u32;
        functions.push(HelperFn {
            params: vec![ValType::I32, ValType::I32, ValType::F64],
            results: vec![ValType::F64],
            body,
        });
        idx
    };
    let lookup_interp = push_lookup(&mut functions, super::lookup::emit_lookup_interp(approx_eq));
    let lookup_forward = push_lookup(&mut functions, super::lookup::emit_lookup_forward());
    let lookup_backward = push_lookup(&mut functions, super::lookup::emit_lookup_backward());

    BuiltHelpers {
        fns: HelperFns {
            approx_eq,
            mod_euclid,
            pulse,
            exp,
            ln,
            sin,
            cos,
            tan,
            atan,
            asin,
            acos,
            log10,
            pow,
            lookup_interp,
            lookup_forward,
            lookup_backward,
        },
        functions,
    }
}

// `approx_eq` helper local layout. Params 0/1 are `a`/`b`; the rest are declared
// i64 scratch locals.
const AE_A: u32 = 0;
const AE_B: u32 = 1;
const AE_BITS: u32 = 2; // scratch for one operand's raw bits
const AE_ORD_A: u32 = 3; // ordered(a)
const AE_ORD_B: u32 = 4; // ordered(b)
const AE_DIFF: u32 = 5; // ordered(a) - ordered(b)
const AE_ABS: u32 = 6; // abs(diff) before saturation
const AE_LOCAL_COUNT: u32 = 5; // declared i64 locals (indices 2..=6)

/// Build the body of the `approx_eq(a: f64, b: f64) -> i32` helper, reproducing
/// `crate::float::approx_eq` (`float_cmp` 0.10, `f64`, default margin
/// `epsilon = f64::EPSILON`, `ulps = 4`) bit-faithfully.
///
/// The Rust reference (`float_cmp` `eq.rs`) is the short-circuiting OR of three
/// total, trap-free checks (exact equality / ±inf, absolute-epsilon, ULP):
///
/// ```text
/// a == b  ||  f64abs(a - b) <= f64::EPSILON  ||  saturating_abs(ulps(a, b)) <= 4
/// ```
///
/// where `ulps(a, b) = ordered(a).wrapping_sub(ordered(b))` over `i64` and
/// `ordered(f) = { let bits = f.to_bits() as i64; if bits < 0 { !bits } else
/// { bits ^ i64::MIN } }` maps the sign-magnitude bit pattern to a monotonic
/// integer. Because all three checks are pure and total (no division, no traps),
/// evaluating them eagerly and OR-ing the i32 results is bit-identical to the
/// Rust short-circuit; the fast path is only a performance shortcut, not a
/// semantic difference. Notably this makes `approx_eq(NaN, NaN) == true`
/// (identical bits -> 0 ULPs) and keeps the finite `crate::float::NA` sentinel
/// distinct from ordinary values (its exponent is far from theirs).
fn emit_approx_eq() -> Function {
    use Instruction as Ins;
    let mut f = Function::new([(AE_LOCAL_COUNT, ValType::I64)]);

    // check 1: a == b -> i32
    f.instruction(&Ins::LocalGet(AE_A));
    f.instruction(&Ins::LocalGet(AE_B));
    f.instruction(&Ins::F64Eq);

    // check 2: f64.abs(a - b) <= f64::EPSILON -> i32
    f.instruction(&Ins::LocalGet(AE_A));
    f.instruction(&Ins::LocalGet(AE_B));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(f64::EPSILON));
    f.instruction(&Ins::F64Le);

    // check 3: saturating_abs(ordered(a) - ordered(b)) <= 4 -> i32.
    emit_ordered_bits(&mut f, AE_A, AE_BITS);
    f.instruction(&Ins::LocalSet(AE_ORD_A));
    emit_ordered_bits(&mut f, AE_B, AE_BITS);
    f.instruction(&Ins::LocalSet(AE_ORD_B));

    // diff = wrapping_sub(ordered_a, ordered_b)  (i64.sub wraps)
    f.instruction(&Ins::LocalGet(AE_ORD_A));
    f.instruction(&Ins::LocalGet(AE_ORD_B));
    f.instruction(&Ins::I64Sub);
    f.instruction(&Ins::LocalSet(AE_DIFF));

    // abs = if diff < 0 { 0 - diff } else { diff }  (the wrapping negate; for
    // diff == i64::MIN this stays negative, handled by the saturation below).
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::LocalGet(AE_DIFF));
    f.instruction(&Ins::I64Sub); // 0 - diff
    f.instruction(&Ins::LocalGet(AE_DIFF)); // [neg, diff]
    f.instruction(&Ins::LocalGet(AE_DIFF));
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::I64LtS); // diff < 0
    f.instruction(&Ins::Select); // neg if diff<0 else diff
    f.instruction(&Ins::LocalSet(AE_ABS));

    // sat = if abs < 0 { i64::MAX } else { abs }  (saturating_abs: the only abs
    // still negative is the i64::MIN overflow, which saturates to i64::MAX).
    f.instruction(&Ins::I64Const(i64::MAX));
    f.instruction(&Ins::LocalGet(AE_ABS)); // [i64::MAX, abs]
    f.instruction(&Ins::LocalGet(AE_ABS));
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::I64LtS); // abs < 0
    f.instruction(&Ins::Select); // i64::MAX if abs<0 else abs

    // sat <= 4 -> i32
    f.instruction(&Ins::I64Const(4));
    f.instruction(&Ins::I64LeS);

    // Combine the three i32 booleans: (check1 | check2 | check3). Stack holds
    // [c1, c2, c3]; two i32.or reduce it to one result.
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::I32Or);

    f.instruction(&Ins::End);
    f
}

/// Append the wasm sequence that pushes `ordered(local)` onto the stack, where
/// `ordered(f) = { let bits = f.to_bits() as i64; if bits < 0 { !bits } else
/// { bits ^ i64::MIN } }` (float_cmp's sign-magnitude -> monotonic map). `bits`
/// is materialized once into `bits_local` (i64) and reused for the two branch
/// values and the sign test; `select` chooses between them. `i64::MIN` is the
/// `1 << 63` sign mask as a signed `i64`, and `!bits` is `bits ^ -1`.
fn emit_ordered_bits(f: &mut Function, src_local: u32, bits_local: u32) {
    use Instruction as Ins;
    f.instruction(&Ins::LocalGet(src_local));
    f.instruction(&Ins::I64ReinterpretF64);
    f.instruction(&Ins::LocalSet(bits_local));
    // neg case: !bits = bits ^ -1
    f.instruction(&Ins::LocalGet(bits_local));
    f.instruction(&Ins::I64Const(-1));
    f.instruction(&Ins::I64Xor);
    // pos case: bits ^ i64::MIN  (flip the sign bit)
    f.instruction(&Ins::LocalGet(bits_local));
    f.instruction(&Ins::I64Const(i64::MIN));
    f.instruction(&Ins::I64Xor);
    // cond: bits < 0  (the sign bit is set)
    f.instruction(&Ins::LocalGet(bits_local));
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::I64LtS);
    // select(neg, pos, cond): neg if cond != 0 else pos
    f.instruction(&Ins::Select);
}

// `mod_euclid` helper local layout. Params 0/1 are `l`/`r`; local 2 is the
// truncated remainder `r0`.
const ME_L: u32 = 0;
const ME_R: u32 = 1;
const ME_R0: u32 = 2;

/// Build the body of `mod_euclid(l: f64, r: f64) -> f64`, reproducing
/// `f64::rem_euclid` (the VM's `Op2::Mod`) exactly.
///
/// `rem_euclid` is `let r0 = l % r; if r0 < 0 { r0 + r.abs() } else { r0 }`,
/// where the truncated remainder `l % r` is `l - r * (l / r).trunc()` (wasm has
/// no `f64.rem`, so it is computed from `f64.div`/`f64.trunc`/`f64.mul`/
/// `f64.sub`). The branch is a `select`. The result lies in `[0, |r|)` for a
/// non-zero divisor; this trunc-then-adjust form is correct for negative
/// divisors too (where a `floor`-based form would not be).
fn emit_mod_euclid() -> Function {
    use Instruction as Ins;
    let mut f = Function::new([(1, ValType::F64)]);

    // r0 = l - r * trunc(l / r)
    f.instruction(&Ins::LocalGet(ME_L));
    f.instruction(&Ins::LocalGet(ME_R));
    f.instruction(&Ins::LocalGet(ME_L));
    f.instruction(&Ins::LocalGet(ME_R));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::F64Trunc);
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(ME_R0));

    // select(r0 + |r|, r0, r0 < 0): the adjusted value when r0 is negative,
    // else r0 unchanged. wasm `select` yields the deeper operand when the cond
    // is true, so push `r0 + |r|` first.
    f.instruction(&Ins::LocalGet(ME_R0));
    f.instruction(&Ins::LocalGet(ME_R));
    f.instruction(&Ins::F64Abs);
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalGet(ME_R0));
    f.instruction(&Ins::LocalGet(ME_R0));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::Select);

    f.instruction(&Ins::End);
    f
}

// `pulse` helper local layout. Params 0..4 are time/dt/volume/first_pulse/
// interval; local 5 is the running `next_pulse`.
const PU_TIME: u32 = 0;
const PU_DT: u32 = 1;
const PU_VOLUME: u32 = 2;
const PU_FIRST: u32 = 3;
const PU_INTERVAL: u32 = 4;
const PU_NEXT: u32 = 5;

/// Build the body of `pulse(time, dt, volume, first_pulse, interval) -> f64`,
/// reproducing the VM's `pulse` (`vm.rs:3036`) including its `while` loop.
///
/// ```text
/// if time < first_pulse { return 0.0 }
/// next_pulse = first_pulse
/// loop {                              // while time >= next_pulse
///     if time < next_pulse { break }
///     if time < next_pulse + dt { return volume / dt }
///     if interval <= 0.0 { break }
///     next_pulse += interval
/// }
/// 0.0
/// ```
///
/// The `while time >= next_pulse` head is realized as a `br $exit` when
/// `time < next_pulse`, inside a `block $exit { loop $top { ... br $top } }`.
fn emit_pulse() -> Function {
    use Instruction as Ins;
    use wasm_encoder::BlockType;
    let mut f = Function::new([(1, ValType::F64)]);

    // if time < first_pulse { return 0.0 }
    f.instruction(&Ins::LocalGet(PU_TIME));
    f.instruction(&Ins::LocalGet(PU_FIRST));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // next_pulse = first_pulse
    f.instruction(&Ins::LocalGet(PU_FIRST));
    f.instruction(&Ins::LocalSet(PU_NEXT));

    // block $exit { loop $top { ... } }
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));

    // while-head: if time < next_pulse { break }  (br depth 1 -> $exit)
    f.instruction(&Ins::LocalGet(PU_TIME));
    f.instruction(&Ins::LocalGet(PU_NEXT));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::BrIf(1));

    // if time < next_pulse + dt { return volume / dt }
    f.instruction(&Ins::LocalGet(PU_TIME));
    f.instruction(&Ins::LocalGet(PU_NEXT));
    f.instruction(&Ins::LocalGet(PU_DT));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(PU_VOLUME));
    f.instruction(&Ins::LocalGet(PU_DT));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // else if interval <= 0.0 { break }  (br depth 1 -> $exit)
    f.instruction(&Ins::LocalGet(PU_INTERVAL));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::BrIf(1));

    // else next_pulse += interval ; continue (br depth 0 -> $top)
    f.instruction(&Ins::LocalGet(PU_NEXT));
    f.instruction(&Ins::LocalGet(PU_INTERVAL));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(PU_NEXT));
    f.instruction(&Ins::Br(0));

    f.instruction(&Ins::End); // end loop
    f.instruction(&Ins::End); // end block

    // fell out of the loop -> 0.0
    f.instruction(&f64_const(0.0));

    f.instruction(&Ins::End); // end function
    f
}

/// Push `call approx_eq` for two f64 operands already on the wasm stack
/// (`[a, b]`); leaves an i32 (1 = approximately equal) on the stack. Mirrors a
/// `crate::float::approx_eq(a, b)` call.
fn emit_call_approx_eq(ctx: &EmitCtx, f: &mut Function) {
    f.instruction(&Instruction::Call(ctx.helpers.approx_eq));
}

/// Push the i32 truthiness of the f64 already on the wasm stack, reproducing the
/// VM's `is_truthy(n) = !approx_eq(n, 0.0)` (`vm.rs:89`): `approx_eq(n, 0.0)`
/// gives `is_false`, and `i32.eqz` negates it to `is_truthy`.
fn emit_is_truthy(ctx: &EmitCtx, f: &mut Function) {
    f.instruction(&f64_const(0.0));
    emit_call_approx_eq(ctx, f);
    f.instruction(&Instruction::I32Eqz);
}

/// The maximum number of simultaneously-live `SetCond` condition registers a
/// program needs.
///
/// `compiler::codegen` lowers an `Expr::If` by walking the *condition* sub-tree
/// to completion before emitting the pair's own `SetCond`/`If`
/// (`codegen.rs:1153-1159`: push `t`, push `f`, walk `cond`, then `SetCond`,
/// `If`). So even when a condition itself contains a nested `If`, the inner
/// pair is fully emitted before the outer `SetCond`, and the stream is
/// *sequential* -- `SetCond If SetCond If` -- never interleaved. With current
/// codegen the condition register therefore never needs to hold more than one
/// live value (this returns 1 for any model with a conditional).
///
/// We still model the register as a LIFO stack and size it from the actual
/// opcode stream rather than hard-coding 1: it costs one cheap pass, it is
/// robust if codegen ever emits a genuinely interleaved pair, and it keeps the
/// emitter's `SetCond`-pushes-/`If`-pops logic symmetric. The depth is computed
/// here so the caller can reserve exactly that many wasm locals.
/// Number of dedicated scratch f64 locals the `Apply` opcode reserves
/// (`a`/`b`/`c`).
const APPLY_LOCAL_COUNT: u32 = 3;

/// The local-declaration list for an opcode-program `Function` carrying
/// `cond_depth` condition locals and `extra_i32` dynamic-subscript scratch
/// locals: one scratch f64, then `cond_depth` i32 condition locals, then
/// [`APPLY_LOCAL_COUNT`] f64 `Apply` scratch locals, then `extra_i32` i32 locals
/// (Task 4 dynamic subscripts; 0 when the program has none).
///
/// Defined once (and consumed by both `module.rs`'s function builders and the
/// `#[cfg(test)]` harness) so the declared local *types and order* match the
/// indices [`apply_locals_for`] and [`extra_i32_local_base`] hand out. Param 0
/// is `module_off`. The extra i32s come *last* so they never disturb the
/// `apply_locals` indices.
pub(crate) fn opcode_fn_locals(cond_depth: usize, extra_i32: u32) -> Vec<(u32, ValType)> {
    vec![
        (1, ValType::F64),
        (cond_depth as u32, ValType::I32),
        (APPLY_LOCAL_COUNT, ValType::F64),
        (extra_i32, ValType::I32),
    ]
}

/// First wasm local index of the `extra_i32` dynamic-subscript scratch locals
/// for a function with `cond_depth` condition locals: past param 0
/// (`module_off`), the scratch f64 (index 1), the `cond_depth` i32 condition
/// locals, and the [`APPLY_LOCAL_COUNT`] `Apply` f64s. Threaded into
/// [`EmitCtx::extra_i32_local_base`] so the dynamic-subscript local allocator
/// draws from exactly the declared range.
pub(crate) fn extra_i32_local_base(cond_depth: usize) -> u32 {
    2 + cond_depth as u32 + APPLY_LOCAL_COUNT
}

/// The three `Apply` scratch f64 local indices `[a, b, c]` for a function with
/// `cond_depth` condition locals. They follow param 0 (`module_off`), the
/// scratch f64 (index 1), and the `cond_depth` i32 condition locals, so they
/// start at `2 + cond_depth`. Mirrors the declaration order in
/// [`opcode_fn_locals`].
pub(crate) fn apply_locals_for(cond_depth: usize) -> [u32; 3] {
    let base = 2 + cond_depth as u32; // 1 (param) + 1 (scratch) + cond_depth
    [base, base + 1, base + 2]
}

pub(crate) fn max_condition_depth(bc: &ByteCode) -> usize {
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    for op in &bc.code {
        match op {
            Opcode::SetCond {} => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            // `If` consumes the most-recently-set condition. Guard against an
            // unbalanced program (which would indicate malformed bytecode)
            // with a saturating decrement rather than an underflow panic.
            Opcode::If {} => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    max_depth
}

/// Push the dynamic part of a module-relative slot address: `module_off * 8`.
/// Combined with a constant `memarg.offset` of `chunk_base + off*8`, this
/// addresses `chunk_base + (module_off + off) * 8`, matching the VM's
/// `curr[module_off + off]` / `next[module_off + off]`.
fn push_module_relative_base(ctx: &EmitCtx, f: &mut Function) {
    f.instruction(&Instruction::LocalGet(ctx.module_off_local));
    f.instruction(&Instruction::I32Const(SLOT_SIZE as i32));
    f.instruction(&Instruction::I32Mul);
}

/// Byte offset of a slot within a chunk: `chunk_base + off*8`.
fn slot_byte_offset(chunk_base: u32, off: u16) -> u64 {
    u64::from(chunk_base) + u64::from(off) * u64::from(SLOT_SIZE)
}

/// Emit-time analogue of the VM's per-`eval_bytecode` mutable state
/// (`vm.rs:1277-1288`): the compile-time view stack, the iteration / broadcast
/// contexts, and the condition-register stack pointer. Threaded through
/// [`emit_ops`] so an unrolled iteration body can be re-emitted at each
/// compile-time index without re-deriving the view stack.
struct EmitState {
    /// Emit-time stack pointer into `ctx.condition_locals`, mirroring the VM's
    /// single `condition` register but generalized to nested `If`s.
    cond_sp: usize,
    /// Compile-time analogue of the VM's runtime `view_stack`: the `Push*View` /
    /// `View*` opcodes push/transform/pop `ViewDesc`s here, and the reducers read
    /// the top descriptor. Because every static view's geometry is known at
    /// compile time, this never materializes anything at runtime -- element
    /// addresses are folded into the emitted reads.
    view_stack: Vec<ViewDesc>,
    /// Active (unrolled) iteration contexts, one per nested `BeginIter`. The
    /// `current` field is the compile-time iteration index the unroller is
    /// emitting (Task 3).
    iter_stack: Vec<IterCtx>,
    /// Active broadcast-iteration contexts (`BeginBroadcastIter`, Task 3).
    broadcast_stack: Vec<BroadcastCtx>,
    /// The legacy scalar dynamic-subscript accumulator (`PushSubscriptIndex` /
    /// `LoadSubscript`, Task 4), mirroring the VM's `subscript_index` +
    /// `subscript_index_valid` (`vm.rs:1287-1288`). Cleared by each
    /// `LoadSubscript`.
    subscript: SubscriptAccum,
    /// Bump cursor for the function's extra i32 locals (Task 4). A dynamic
    /// subscript draws fresh i32 locals from here; the count is pre-sized by
    /// [`count_extra_i32_locals`], so this never exceeds the declared count.
    next_i32_local: u32,
    /// Cumulative count of unrolled element-emit "units" for the function being
    /// lowered, checked against [`MAX_UNROLL_UNITS`] (see [`EmitState::charge_unroll`]).
    /// Every full unroll -- a reducer fold, a `BeginIter`/`BeginBroadcastIter`
    /// body re-emission -- charges its iteration count here. Nested iterations
    /// multiply naturally: an inner site is reached once per outer iteration, so
    /// each inner charge already reflects the outer multiplier. When the running
    /// total would exceed the cap, lowering aborts with `Unsupported` so the
    /// model cleanly falls back to the VM instead of emitting a multi-megabyte
    /// function body that a wasm engine would reject.
    unroll_units: usize,
}

/// Upper bound on the cumulative number of unrolled element-emit "units" per
/// wasm function (one reducer-fold element, or one `BeginIter`/`BeginBroadcastIter`
/// body re-emission, is one unit).
///
/// Every array reducer and iteration loop is fully unrolled at compile time
/// (each element address becomes a wasm constant -- see [`emit_reduce_fold`] and
/// the `BeginIter`/`BeginBroadcastIter` arms). Without a bound, a large arrayed
/// model -- especially nested iterations whose counts multiply -- could emit a
/// function body exceeding what wasm engines accept (V8, for instance, caps a
/// single function near ~7.6 MB of bytecode; the spec's 4 GiB ceiling is
/// academic). At a generous ~50 bytes of emitted code per unit, this cap bounds
/// unroll-driven code at roughly 3 MB, comfortably under the strictest engine
/// limit.
///
/// The value `65_536` (2^16) is the natural ceiling of a single `u16` array
/// dimension (`ViewDesc::dims` entries are `u16`, so one dimension tops out at
/// 65_535). Real system-dynamics arrays are tiny -- the test corpus's largest
/// single dimension is 9, and even a region x sector x cohort nest is on the
/// order of 10^3 elements -- so this leaves >60x headroom for legitimate models
/// while rejecting pathological products (e.g. a `[300, 300]` view, 90_000
/// elements) before any code is emitted.
///
/// future: a runtime wasm loop driven by a precomputed offset table (per the
/// Phase 5 design's non-contiguous path) would lift this cap entirely, trading a
/// constant-size loop body for the current fully-unrolled form.
const MAX_UNROLL_UNITS: usize = 65_536;

impl EmitState {
    /// Charge `units` against the per-function unroll budget, returning
    /// `Unsupported` (so the model falls back to the VM) if the running total
    /// would exceed [`MAX_UNROLL_UNITS`]. Called *before* an unroll site emits
    /// its body, so an over-budget model is rejected without ever materializing
    /// the oversized function. `units` saturates rather than wrapping, so a
    /// pathological multi-dimensional product can never alias back under the cap.
    fn charge_unroll(&mut self, units: usize) -> Result<(), WasmGenError> {
        self.unroll_units = self.unroll_units.saturating_add(units);
        if self.unroll_units > MAX_UNROLL_UNITS {
            return Err(WasmGenError::Unsupported(format!(
                "wasmgen: array unrolling exceeds the per-function budget of \
                 {MAX_UNROLL_UNITS} elements (a large arrayed model); falling back to the VM"
            )));
        }
        Ok(())
    }
}

/// The legacy scalar dynamic-subscript accumulator (Task 4). `PushSubscriptIndex`
/// appends a `(runtime_index_local, bounds)` and folds OOB into `valid_local`;
/// `LoadSubscript` collapses the indices into a flat offset and reads the slot
/// (or NaN). Mirrors the VM's `subscript_index` SmallVec + `subscript_index_valid`
/// flag (`vm.rs:1287-1288`, `1341-1366`).
#[derive(Default)]
struct SubscriptAccum {
    /// `(runtime_index_local, bounds)` for each pushed index, in push order. The
    /// local holds the *0-based* runtime index (i32); `bounds` is the dimension
    /// size for the row-major fold.
    indices: Vec<(u32, u16)>,
    /// wasm i32 local that is 0 once any pushed index was out of bounds, else 1.
    /// `None` until the first `PushSubscriptIndex` of an accumulation allocates
    /// it (then seeded to 1).
    valid_local: Option<u32>,
}

/// One active iteration context for the unrolled `BeginIter` loop (Task 3).
struct IterCtx {
    /// The view captured as the iteration source/geometry at `BeginIter`
    /// (`view_stack.last()` then).
    iter_view: ViewDesc,
    /// Destination temp id for `StoreIterElement`, when `has_write_temp`.
    write_temp_id: Option<u8>,
    /// The compile-time iteration index currently being emitted (the unroller
    /// re-emits the body once per `0..size`).
    current: usize,
}

/// One active broadcast-iteration context (`BeginBroadcastIter`, Task 3),
/// mirroring the VM's `BroadcastState` (`vm.rs:68-81`) but with the result
/// geometry + per-source dim maps resolved at compile time.
struct BroadcastCtx {
    /// Per source (deepest-first): the source view and its `dim_map` (one entry
    /// per result dimension; `Some(src_dim)` or `None` for a broadcast axis).
    sources: Vec<(ViewDesc, Vec<Option<usize>>)>,
    /// Destination temp id for `StoreBroadcastElement`.
    dest_temp_id: u8,
    /// Result dimension sizes (the union of all sources' dims, first-encounter
    /// order), used to decompose `current` into per-result-dim indices.
    result_dims: Vec<u16>,
    /// The compile-time result index currently being emitted.
    current: usize,
}

/// Lower one opcode program. Value-producing opcodes leave their f64 result on
/// the wasm operand stack; the assignment opcodes emit a store and leave the
/// stack empty, exactly as the VM's stack-machine arms do. `Ret` is a no-op
/// here: the wasm function's terminating `End` is emitted by the caller.
pub(crate) fn emit_bytecode(
    bc: &ByteCode,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let mut state = EmitState {
        cond_sp: 0,
        view_stack: Vec::new(),
        iter_stack: Vec::new(),
        broadcast_stack: Vec::new(),
        subscript: SubscriptAccum::default(),
        next_i32_local: ctx.extra_i32_local_base,
        unroll_units: 0,
    };
    emit_ops(&bc.code, &bc.literals, ctx, &mut state, f)
}

/// An upper bound on the extra i32 wasm locals a program's dynamic subscripts
/// need (Task 4), so the function-builder can reserve them past the scratch /
/// condition / `Apply` locals.
///
/// Each `ViewSubscriptDynamic` draws at most two fresh locals (a runtime-offset
/// addend + a validity flag, allocated once per dynamically-subscripted view);
/// each `PushSubscriptIndex` draws at most two (a 0-based index local + the
/// shared validity flag of its accumulation). Counting two per opcode is a
/// generous bound -- a real accumulation reuses one view's pair across several
/// subscripts and one validity flag across several pushed indices -- but
/// over-reserving unused i32 locals is free, and the bound keeps the reservation
/// a single cheap pass with no dataflow.
pub(crate) fn count_extra_i32_locals(bc: &ByteCode) -> u32 {
    bc.code
        .iter()
        .filter(|op| {
            matches!(
                op,
                Opcode::ViewSubscriptDynamic { .. } | Opcode::PushSubscriptIndex { .. }
            )
        })
        .count() as u32
        * 2
}

/// Lower a (sub-)slice of opcodes, threading the emit-time [`EmitState`]. The
/// top-level program is one call over the whole `code`; an unrolled `BeginIter`
/// loop body (Task 3) re-enters here over the body sub-slice once per iteration
/// index. A `pc`-based loop (rather than `for`) lets the iteration arms consume
/// their structured `BeginIter..NextIterOrJump..EndIter` span and re-emit the
/// body, mirroring the VM's `pc` loop without needing the `jump_back` delta.
///
/// `literals` is the program's shared literal pool (`LoadConstant` /
/// `AssignConstCurr` index it); it is the same across every body re-emission.
fn emit_ops(
    code: &[Opcode],
    literals: &[f64],
    ctx: &EmitCtx,
    state: &mut EmitState,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let mut pc = 0usize;
    while pc < code.len() {
        let op = &code[pc];
        match op {
            Opcode::LoadConstant { id } => {
                let v = *literals.get(*id as usize).ok_or_else(|| {
                    WasmGenError::Unsupported(format!(
                        "wasmgen: LoadConstant literal id {id} out of range"
                    ))
                })?;
                f.instruction(&f64_const(v));
            }
            Opcode::LoadVar { off } => {
                push_module_relative_base(ctx, f);
                f.instruction(&Instruction::F64Load(memarg(slot_byte_offset(
                    ctx.curr_base,
                    *off,
                ))));
            }
            Opcode::LoadGlobalVar { off } => {
                // Absolute slot: ignore module_off (slots 0..4 are global).
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::F64Load(memarg(slot_byte_offset(
                    ctx.curr_base,
                    *off,
                ))));
            }
            Opcode::Op2 { op } => emit_op2(*op, ctx, f)?,
            Opcode::Not {} => {
                // The VM's `Not` is `(!is_truthy(r)) as f64`, which simplifies to
                // `approx_eq(r, 0.0) as f64` (since `is_truthy = !approx_eq(·,0.0)`,
                // the double negation cancels). So push `approx_eq(r, 0.0)` and
                // widen the i32 1/0 to f64.
                f.instruction(&f64_const(0.0));
                emit_call_approx_eq(ctx, f);
                f.instruction(&Instruction::F64ConvertI32U);
            }
            Opcode::SetCond {} => {
                let local = *ctx.condition_locals.get(state.cond_sp).ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: SetCond nesting exceeded reserved condition locals".to_string(),
                    )
                })?;
                // Reduce the f64 condition to i32 truthiness, routing through
                // `approx_eq` so a near-zero / ULP-adjacent condition takes the
                // same branch the VM's `is_truthy(pop)` takes.
                emit_is_truthy(ctx, f);
                f.instruction(&Instruction::LocalSet(local));
                state.cond_sp += 1;
            }
            Opcode::If {} => {
                if state.cond_sp == 0 {
                    return Err(WasmGenError::Unsupported(
                        "wasmgen: If without a preceding SetCond".to_string(),
                    ));
                }
                state.cond_sp -= 1;
                let local = ctx.condition_locals[state.cond_sp];
                // Stack holds [t, f] (the VM pops f then t and yields
                // `if condition { t } else { f }`); wasm `select` pops
                // [t, f, cond_i32] and yields t when cond != 0 else f -- exact.
                f.instruction(&Instruction::LocalGet(local));
                f.instruction(&Instruction::Select);
            }
            Opcode::AssignCurr { off } => {
                emit_assign(ctx.curr_base, *off, ctx, f);
            }
            Opcode::AssignNext { off } => {
                emit_assign(ctx.next_base, *off, ctx, f);
            }
            // `AssignConstCurr` reaches a `CompiledSimulation` by two routes
            // (see the module docstring): `compiler::codegen` emits it directly
            // for any constant-RHS `AssignCurr` (`codegen.rs:1164`), and the
            // peephole pass also fuses `LoadConstant; AssignCurr` into it
            // (`bytecode.rs:1830`). It is *not* a late-3-address fusion artifact,
            // so it is part of the scalar core, not an Unsupported case. Every
            // model with a constant initial/aux carries it. Mirrors the VM's
            // `curr[module_off + off] = literals[literal_id]` (`vm.rs:1453`).
            Opcode::AssignConstCurr { off, literal_id } => {
                let v = *literals.get(*literal_id as usize).ok_or_else(|| {
                    WasmGenError::Unsupported(format!(
                        "wasmgen: AssignConstCurr literal id {literal_id} out of range"
                    ))
                })?;
                // Nothing is on the stack; push the store address then the
                // constant value (f64.store wants [addr_i32, value_f64]).
                push_module_relative_base(ctx, f);
                f.instruction(&f64_const(v));
                f.instruction(&Instruction::F64Store(memarg(slot_byte_offset(
                    ctx.curr_base,
                    *off,
                ))));
            }
            // Peephole fusions of `Op2; Assign{Curr,Next}`. Operands `[l, r]`
            // are on the stack; apply the op (which errors cleanly on an
            // unsupported operator) then store the result. Mirrors the VM's
            // `curr/next[module_off + off] = eval_op2(op, l, r)` (`vm.rs:1457`,
            // `vm.rs:1463`).
            Opcode::BinOpAssignCurr { op, off } => {
                emit_op2(*op, ctx, f)?;
                emit_assign(ctx.curr_base, *off, ctx, f);
            }
            Opcode::BinOpAssignNext { op, off } => {
                emit_op2(*op, ctx, f)?;
                emit_assign(ctx.next_base, *off, ctx, f);
            }
            // `Apply` always pops exactly three operands (codegen pads short
            // builtins with `LoadConstant 0.0` / `LoadGlobalVar{FINAL_TIME}`),
            // mirroring the VM (`vm.rs:1701`). See [`emit_apply`].
            Opcode::Apply { func } => emit_apply(*func, ctx, f),
            // `Lookup` pops `index` then `element_offset`, bounds-checks the
            // offset, and dispatches to the mode's helper over the table at
            // `base_gf + element_offset` (`vm.rs:1710`). See [`emit_lookup`].
            Opcode::Lookup {
                base_gf,
                table_count,
                mode,
            } => emit_lookup(*base_gf, *table_count, *mode, ctx, f),
            // `LoadPrev` mirrors the VM (`vm.rs:1320-1328`): a fallback is
            // already on the stack (codegen pushes it just before this opcode);
            // yield it while `use_prev_fallback` is set, otherwise read
            // `prev_values[module_off + off]`. The gate is the global flag, never
            // a TIME comparison (RK moves TIME to trial points).
            Opcode::LoadPrev { off } => emit_load_prev(*off, ctx, f),
            // `LoadInitial` mirrors the VM (`vm.rs:1332-1340`), but its
            // `part == Initials` branch is resolved at compile time from
            // `ctx.step_part`: in the initials program read `curr[module_off+off]`
            // (the value being computed); elsewhere read the post-initials
            // `initial_values[module_off+off]` snapshot.
            Opcode::LoadInitial { off } => emit_load_initial(*off, ctx, f),

            // ── View-stack construction (Phase 5 Task 1) ──────────────────
            // Each opcode pushes/transforms a compile-time `ViewDesc`, mirroring
            // the VM's `view_stack` arms (`vm.rs:1739-1855`). No wasm is emitted:
            // the geometry is folded into later element reads.
            Opcode::PushStaticView { view_id } => {
                let view = ctx.ctx.get_static_view(*view_id).ok_or_else(|| {
                    WasmGenError::Unsupported(format!(
                        "wasmgen: PushStaticView view_id {view_id} out of range"
                    ))
                })?;
                state.view_stack.push(ViewDesc::from_static(view));
            }
            // `PushVarView` builds a full contiguous view over a variable array;
            // the VM folds `module_off` into the base (`vm.rs:1749`), so the base
            // is module-relative.
            Opcode::PushVarView {
                base_off,
                dim_list_id,
            } => {
                let (dims, dim_ids) = resolve_dim_list_dims(ctx, *dim_list_id)?;
                state.view_stack.push(ViewDesc::contiguous(
                    u32::from(*base_off),
                    ViewBase::CurrModuleRelative,
                    dims,
                    dim_ids,
                ));
            }
            // `PushTempView` builds a full contiguous view over a temp array
            // (`vm.rs:1757`).
            Opcode::PushTempView {
                temp_id,
                dim_list_id,
            } => {
                let (dims, dim_ids) = resolve_dim_list_dims(ctx, *dim_list_id)?;
                state.view_stack.push(ViewDesc::contiguous(
                    u32::from(*temp_id),
                    ViewBase::Temp,
                    dims,
                    dim_ids,
                ));
            }
            // `PushVarViewDirect` builds a contiguous view from raw dim sizes
            // (dim_ids all 0), the base for a dynamic subscript (`vm.rs:1776`).
            // Module-relative, like `PushVarView`.
            Opcode::PushVarViewDirect {
                base_off,
                dim_list_id,
            } => {
                let dims = resolve_dim_list_raw(ctx, *dim_list_id)?;
                let n = dims.len();
                state.view_stack.push(ViewDesc::contiguous(
                    u32::from(*base_off),
                    ViewBase::CurrModuleRelative,
                    dims,
                    vec![0u16; n],
                ));
            }

            // ── View-stack transforms (Phase 5 Task 1) ────────────────────
            Opcode::ViewSubscriptConst { dim_idx, index } => {
                view_top_mut(&mut state.view_stack)?
                    .apply_single_subscript(*dim_idx as usize, *index);
            }
            Opcode::ViewRange {
                dim_idx,
                start,
                end,
            } => {
                view_top_mut(&mut state.view_stack)?.apply_range(*dim_idx as usize, *start, *end);
            }
            Opcode::ViewStarRange {
                dim_idx,
                subdim_relation_id,
            } => {
                let rel = ctx
                    .ctx
                    .subdim_relations
                    .get(*subdim_relation_id as usize)
                    .ok_or_else(|| {
                        WasmGenError::Unsupported(format!(
                            "wasmgen: ViewStarRange subdim_relation_id {subdim_relation_id} \
                             out of range"
                        ))
                    })?;
                let parent_offsets = rel.parent_offsets.to_vec();
                let child_dim_id = rel.child_dim_id;
                view_top_mut(&mut state.view_stack)?.apply_sparse(
                    *dim_idx as usize,
                    parent_offsets,
                    child_dim_id,
                );
            }
            // `ViewWildcard` is a no-op in the VM (`vm.rs:1839`): the dimension
            // stays as-is.
            Opcode::ViewWildcard { dim_idx: _ } => {}
            Opcode::ViewTranspose {} => {
                view_top_mut(&mut state.view_stack)?.transpose();
            }
            Opcode::PopView {} => {
                state.view_stack.pop().ok_or_else(|| {
                    WasmGenError::Unsupported("wasmgen: PopView on empty view stack".to_string())
                })?;
            }
            Opcode::DupView {} => {
                let top = view_top(&state.view_stack)?.clone();
                state.view_stack.push(top);
            }

            // ── Dynamic view subscript (Phase 5 Task 4) ───────────────────
            // `ViewSubscriptDynamic` pops a 1-based runtime index, bounds-checks
            // it against the top view's `dims[dim_idx]`, and folds
            // `(index-1)*strides[dim_idx]` into the descriptor's runtime offset
            // local; OOB sets the validity flag to 0 so later reads yield NaN.
            // Mirrors `RuntimeView::apply_single_subscript_checked` (`vm.rs:1797`,
            // `bytecode.rs:242`).
            Opcode::ViewSubscriptDynamic { dim_idx } => {
                emit_view_subscript_dynamic(*dim_idx as usize, ctx, state, f)?;
            }
            // `ViewRangeDynamic` (`vm.rs:1815`) clamps a runtime `[start:end]`
            // range, which yields a runtime *size*. The unrolled element
            // addressing here folds every address at compile time, so a runtime
            // range cannot be expressed; returning `Unsupported` keeps such a
            // model `Skipped`. A literal range is constant-folded by codegen into
            // the static `ViewRange` arm, so this is only reached by a true
            // runtime range.
            Opcode::ViewRangeDynamic { dim_idx } => {
                return Err(WasmGenError::Unsupported(format!(
                    "wasmgen: ViewRangeDynamic (dim {dim_idx}) needs a runtime view size; \
                     not supported"
                )));
            }

            // ── Legacy scalar dynamic subscript (Phase 5 Task 4) ──────────
            // `PushSubscriptIndex` pops a 1-based index, range-checks it against
            // `bounds`, and accumulates the 0-based runtime index; OOB clears the
            // accumulation's validity flag. `LoadSubscript` folds the accumulated
            // indices into a flat offset and reads `curr[module_off+off+flat]`
            // (NaN when invalid). Mirrors `vm.rs:1341-1366`.
            Opcode::PushSubscriptIndex { bounds } => {
                emit_push_subscript_index(*bounds, state, f);
            }
            Opcode::LoadSubscript { off } => {
                emit_load_subscript(*off, ctx, state, f);
            }

            // ── Temp element reads (Phase 5 Task 1) ───────────────────────
            // `temp_storage[temp_offsets[temp_id] + index]` (`vm.rs:1860`).
            Opcode::LoadTempConst { temp_id, index } => {
                let addr = temp_element_byte_addr(ctx, *temp_id, u32::from(*index))?;
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::F64Load(memarg(addr)));
            }
            // `temp_storage[temp_offsets[temp_id] + index]` with a runtime index
            // (`vm.rs:1866`): the VM does `stack.pop().floor() as usize`.
            Opcode::LoadTempDynamic { temp_id } => {
                emit_load_temp_dynamic(ctx, *temp_id, f)?;
            }

            // ── Array reducers (Phase 5 Task 2) ───────────────────────────
            // Reduce over the TOP view descriptor (the production pattern is
            // `PushStaticView; Array<Reduce>; PopView`, so the descriptor stays
            // for the trailing `PopView`).
            Opcode::ArraySum {}
            | Opcode::ArrayMax {}
            | Opcode::ArrayMin {}
            | Opcode::ArrayMean {}
            | Opcode::ArrayStddev {}
            | Opcode::ArraySize {} => {
                let view = view_top(&state.view_stack)?.clone();
                // `ArraySize` emits no element reads (just `size() as f64`), so it
                // is free; every other reducer unrolls a fold over `size()`
                // elements, and `ArrayStddev` makes two passes (sum, then squared
                // deviations). Charge that many units before emitting the fold.
                if !matches!(op, Opcode::ArraySize {}) {
                    let passes = if matches!(op, Opcode::ArrayStddev {}) {
                        2
                    } else {
                        1
                    };
                    state.charge_unroll(view.size().saturating_mul(passes))?;
                }
                emit_array_reduce(op, &view, ctx, f)?;
            }

            // ── Body element reads inside an unrolled iteration (Task 3) ───
            // Each reads view element `current` (the compile-time iteration index
            // the unroller set on the active iter context) and pushes the f64.
            Opcode::LoadIterElement {} => {
                let iter = state.iter_stack.last().ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: LoadIterElement outside an iteration".to_string(),
                    )
                })?;
                // The iteration view is also the source: read element `current`.
                let view = iter.iter_view.clone();
                let current = iter.current;
                emit_view_element_load(&view, current, ctx, f)?;
            }
            // `temp_storage[temp_offsets[temp_id] + current]` (`vm.rs:1939`).
            Opcode::LoadIterTempElement { temp_id } => {
                let current = current_iter_index(state)?;
                let addr = temp_element_byte_addr(ctx, *temp_id, current as u32)?;
                f.instruction(&Instruction::I32Const(0));
                f.instruction(&Instruction::F64Load(memarg(addr)));
            }
            // Read `view_stack.last()` at `current`, broadcasting against the
            // iteration view (`vm.rs:1946`). `LoadIterViewAt{offset}` reads
            // `view_stack[len-offset]` instead (`vm.rs:2068`).
            Opcode::LoadIterViewTop {} => {
                emit_load_iter_view(state, 1, ctx, f)?;
            }
            Opcode::LoadIterViewAt { offset } => {
                emit_load_iter_view(state, *offset as usize, ctx, f)?;
            }
            // Store the popped value into `temp_storage[temp_offsets[write_temp]
            // + current]` (`vm.rs:2184`).
            Opcode::StoreIterElement {} => {
                let iter = state.iter_stack.last().ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: StoreIterElement outside an iteration".to_string(),
                    )
                })?;
                let write_temp_id = iter.write_temp_id.ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: StoreIterElement without a write temp".to_string(),
                    )
                })?;
                let current = iter.current;
                emit_store_iter_element(ctx, write_temp_id, current, f)?;
            }

            // ── Iteration loop (Task 3): unroll BeginIter..EndIter ────────
            // The body span between `BeginIter` and its `NextIterOrJump` is
            // structured (codegen.rs:1183-1378) and well-nested, so rather than a
            // runtime wasm loop with the `jump_back` PC delta, the body is fully
            // unrolled over the compile-time `size()` -- every element address is
            // then a compile-time constant via `emit_view_element_load`, matching
            // the array reducer's unrolled fold (Task 2) and the VM element-for-
            // element. The captured iter view is `view_stack.last()` at `BeginIter`
            // (`vm.rs:1880`).
            Opcode::BeginIter {
                write_temp_id,
                has_write_temp,
            } => {
                let iter_view = view_top(&state.view_stack)?.clone();
                let write_temp_id = if *has_write_temp {
                    Some(*write_temp_id)
                } else {
                    None
                };
                let size = iter_view.size();
                let (body, end_pc) = iter_span(code, pc, IterKind::Iter)?;
                // Re-emitting the body once per element is `size` units of
                // unrolling; charge it before the loop so an over-budget model is
                // rejected without materializing the oversized body. Nested
                // iterations multiply naturally: this arm is reached once per
                // outer iteration, so each inner charge already carries the outer
                // multiplier.
                state.charge_unroll(size)?;
                for current in 0..size {
                    state.iter_stack.push(IterCtx {
                        iter_view: iter_view.clone(),
                        write_temp_id,
                        current,
                    });
                    emit_ops(body, literals, ctx, state, f)?;
                    state.iter_stack.pop();
                }
                pc = end_pc;
                continue;
            }
            // `NextIterOrJump`/`EndIter` are consumed by the `BeginIter` unroll
            // (the body slice excludes the `NextIterOrJump`, and `pc` is advanced
            // past `EndIter`), so reaching one here means malformed bytecode.
            Opcode::NextIterOrJump { .. } | Opcode::EndIter {} => {
                return Err(WasmGenError::Unsupported(
                    "wasmgen: NextIterOrJump/EndIter without a matching BeginIter".to_string(),
                ));
            }

            // ── Broadcast iteration (Task 3): unroll over the union geometry ──
            // `BeginBroadcastIter` unions the `n_sources` views' dim_ids into the
            // result geometry, building a per-source dim map (`vm.rs:2314`); the
            // body is then unrolled over the result size, mirroring
            // `LoadBroadcastElement` / `StoreBroadcastElement`.
            Opcode::BeginBroadcastIter {
                n_sources,
                dest_temp_id,
            } => {
                let bctx = build_broadcast_ctx(state, *n_sources as usize, *dest_temp_id)?;
                let size: usize = bctx.result_dims.iter().map(|&d| d as usize).product();
                let (body, end_pc) = iter_span(code, pc, IterKind::Broadcast)?;
                // Same unroll accounting as `BeginIter`: the body is re-emitted
                // once per element of the broadcast result geometry.
                state.charge_unroll(size)?;
                for current in 0..size {
                    state.broadcast_stack.push(BroadcastCtx {
                        sources: bctx.sources.clone(),
                        dest_temp_id: bctx.dest_temp_id,
                        result_dims: bctx.result_dims.clone(),
                        current,
                    });
                    emit_ops(body, literals, ctx, state, f)?;
                    state.broadcast_stack.pop();
                }
                pc = end_pc;
                continue;
            }
            Opcode::LoadBroadcastElement { source_idx } => {
                emit_load_broadcast_element(state, *source_idx as usize, ctx, f)?;
            }
            Opcode::StoreBroadcastElement {} => {
                let bc_ctx = state.broadcast_stack.last().ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: StoreBroadcastElement outside a broadcast iteration".to_string(),
                    )
                })?;
                let dest_temp_id = bc_ctx.dest_temp_id;
                let current = bc_ctx.current;
                emit_store_iter_element(ctx, dest_temp_id, current, f)?;
            }
            Opcode::NextBroadcastOrJump { .. } | Opcode::EndBroadcastIter {} => {
                return Err(WasmGenError::Unsupported(
                    "wasmgen: NextBroadcastOrJump/EndBroadcastIter without a matching \
                     BeginBroadcastIter"
                        .to_string(),
                ));
            }

            Opcode::Ret => {
                // The caller emits the function's terminating `End`.
            }
            other => return Err(WasmGenError::Unsupported(unsupported_opcode(other))),
        }
        pc += 1;
    }
    Ok(())
}

impl EmitState {
    /// Hand out the next fresh i32 wasm local (Task 4 dynamic subscripts). The
    /// count is pre-reserved by [`count_extra_i32_locals`], so this never exceeds
    /// the function's declared locals.
    fn alloc_i32_local(&mut self) -> u32 {
        let idx = self.next_i32_local;
        self.next_i32_local += 1;
        idx
    }
}

/// The compile-time iteration index of the innermost active iteration context,
/// erroring on a body opcode that appeared outside any iteration.
fn current_iter_index(state: &EmitState) -> Result<usize, WasmGenError> {
    state.iter_stack.last().map(|it| it.current).ok_or_else(|| {
        WasmGenError::Unsupported("wasmgen: iteration body opcode outside an iteration".to_string())
    })
}

/// Which structured iteration the body span belongs to: a `BeginIter` loop or a
/// `BeginBroadcastIter` loop. Each has its own begin/next/end opcode triple, but
/// the well-nested span scan is identical.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IterKind {
    Iter,
    Broadcast,
}

/// Given the `pc` of a `BeginIter` / `BeginBroadcastIter`, return the body slice
/// (the opcodes after the begin, up to but excluding its `NextIterOrJump` /
/// `NextBroadcastOrJump`) and the pc *after* the matching `EndIter` /
/// `EndBroadcastIter` (where the outer loop resumes).
///
/// The span is well-nested (codegen always emits `begin .. next .. end`), so a
/// nested loop of the *same* kind is skipped by depth tracking: `begin` raises
/// the depth and `end` lowers it; the matching `next` is the one at depth 0.
/// A loop of the *other* kind cannot appear inside (codegen never interleaves
/// the two families), but its begin/end would not affect this kind's depth, and
/// its `next` is not this kind's `next`, so the scan is still correct.
fn iter_span(
    code: &[Opcode],
    begin_pc: usize,
    kind: IterKind,
) -> Result<(&[Opcode], usize), WasmGenError> {
    let is_begin = |op: &Opcode| match kind {
        IterKind::Iter => matches!(op, Opcode::BeginIter { .. }),
        IterKind::Broadcast => matches!(op, Opcode::BeginBroadcastIter { .. }),
    };
    let is_next = |op: &Opcode| match kind {
        IterKind::Iter => matches!(op, Opcode::NextIterOrJump { .. }),
        IterKind::Broadcast => matches!(op, Opcode::NextBroadcastOrJump { .. }),
    };
    let is_end = |op: &Opcode| match kind {
        IterKind::Iter => matches!(op, Opcode::EndIter {}),
        IterKind::Broadcast => matches!(op, Opcode::EndBroadcastIter {}),
    };

    let body_start = begin_pc + 1;
    let mut depth = 0usize;
    let mut i = body_start;
    let mut body_end: Option<usize> = None;
    while i < code.len() {
        let op = &code[i];
        if is_begin(op) {
            depth += 1;
        } else if is_next(op) {
            if depth == 0 {
                body_end = Some(i);
                break;
            }
        } else if is_end(op) {
            // `end` closes the most recent nested `begin` of this kind. The
            // outermost (depth-0) `end` is reached only *after* our `next`, so a
            // saturating decrement is safe.
            depth = depth.saturating_sub(1);
        }
        i += 1;
    }
    let body_end = body_end.ok_or_else(|| {
        WasmGenError::Unsupported("wasmgen: iteration with no matching Next opcode".to_string())
    })?;
    // The `end` opcode immediately follows the (depth-0) `next`.
    let end_idx = body_end + 1;
    if end_idx >= code.len() || !is_end(&code[end_idx]) {
        return Err(WasmGenError::Unsupported(
            "wasmgen: iteration Next not immediately followed by End".to_string(),
        ));
    }
    Ok((&code[body_start..body_end], end_idx + 1))
}

/// Lower `LoadIterViewTop` (`stack_offset == 1`) / `LoadIterViewAt { offset }`:
/// read `view_stack[len - stack_offset]` at the innermost iteration's `current`,
/// broadcasting against the captured iteration view (`vm.rs:1946-2182`). An
/// invalid source view, a source smaller than the iteration, or an unmatched
/// dimension pushes NaN, exactly as the VM does.
fn emit_load_iter_view(
    state: &EmitState,
    stack_offset: usize,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let iter = state.iter_stack.last().ok_or_else(|| {
        WasmGenError::Unsupported("wasmgen: LoadIterView* outside an iteration".to_string())
    })?;
    if stack_offset == 0 || stack_offset > state.view_stack.len() {
        return Err(WasmGenError::Unsupported(
            "wasmgen: LoadIterView* stack offset out of range".to_string(),
        ));
    }
    let source = &state.view_stack[state.view_stack.len() - stack_offset];
    // The broadcast index mapping is resolved at compile time; `None` means the
    // VM would push NaN for this (source-element, iteration-index) pair.
    match source.iter_broadcast_offset(&iter.iter_view, iter.current, ctx.ctx) {
        Some(flat) => emit_view_offset_load(source, flat, ctx, f),
        None => {
            f.instruction(&f64_const(f64::NAN));
            Ok(())
        }
    }
}

/// Store the f64 already on the wasm stack into `temp_storage[temp_offsets[
/// temp_id] + index]` (the `StoreIterElement` / `StoreBroadcastElement` write).
/// `f64.store` wants `[addr_i32, value_f64]`, so park the value in the scratch
/// local, push the constant address, then reload the value.
fn emit_store_iter_element(
    ctx: &EmitCtx,
    temp_id: u8,
    index: usize,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let addr = temp_element_byte_addr(ctx, temp_id, index as u32)?;
    f.instruction(&Instruction::LocalSet(ctx.scratch_local));
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::LocalGet(ctx.scratch_local));
    f.instruction(&Instruction::F64Store(memarg(addr)));
    Ok(())
}

/// Build the compile-time broadcast context for a `BeginBroadcastIter`,
/// mirroring the VM's `BeginBroadcastIter` arm (`vm.rs:2314-2373`): union the
/// `n_sources` deepest views' dim_ids into the result geometry (first-encounter
/// order), then build each source's `dim_map` (result dim -> source dim, or
/// `None` for a broadcast axis).
fn build_broadcast_ctx(
    state: &EmitState,
    n_sources: usize,
    dest_temp_id: u8,
) -> Result<BroadcastCtx, WasmGenError> {
    if n_sources == 0 || n_sources > state.view_stack.len() {
        return Err(WasmGenError::Unsupported(
            "wasmgen: BeginBroadcastIter source count out of range".to_string(),
        ));
    }
    let base = state.view_stack.len() - n_sources;
    let sources_slice = &state.view_stack[base..];

    // Result dim ids/sizes: the union over all sources, first-encounter order.
    let mut result_dim_ids: Vec<u16> = Vec::new();
    let mut result_dims: Vec<u16> = Vec::new();
    for view in sources_slice {
        for (d, &dim_id) in view.dim_ids.iter().enumerate() {
            if !result_dim_ids.contains(&dim_id) {
                result_dim_ids.push(dim_id);
                result_dims.push(view.dims[d]);
            }
        }
    }

    // Per source: dim_map[result_dim] = Some(src_dim) by exact dim-id match, else
    // None (the source broadcasts along that axis).
    let mut sources: Vec<(ViewDesc, Vec<Option<usize>>)> = Vec::with_capacity(n_sources);
    for view in sources_slice {
        let dim_map: Vec<Option<usize>> = result_dim_ids
            .iter()
            .map(|&rid| view.dim_ids.iter().position(|&id| id == rid))
            .collect();
        sources.push((view.clone(), dim_map));
    }

    Ok(BroadcastCtx {
        sources,
        dest_temp_id,
        result_dims,
        current: 0,
    })
}

/// Lower `LoadBroadcastElement { source_idx }`, mirroring the VM
/// (`vm.rs:2375-2414`): decompose the broadcast `current` into per-result-dim
/// indices, scatter them into the source's dimension order through its
/// `dim_map`, then read the source element. An invalid source view pushes NaN.
fn emit_load_broadcast_element(
    state: &EmitState,
    source_idx: usize,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let bc_ctx = state.broadcast_stack.last().ok_or_else(|| {
        WasmGenError::Unsupported(
            "wasmgen: LoadBroadcastElement outside a broadcast iteration".to_string(),
        )
    })?;
    let (source, dim_map) = bc_ctx.sources.get(source_idx).ok_or_else(|| {
        WasmGenError::Unsupported(
            "wasmgen: LoadBroadcastElement source_idx out of range".to_string(),
        )
    })?;

    // Decompose the result `current` into per-result-dim indices (row-major).
    let n_result = bc_ctx.result_dims.len();
    let mut result_indices = vec![0u16; n_result];
    let mut remaining = bc_ctx.current;
    for d in (0..n_result).rev() {
        let dim = bc_ctx.result_dims[d] as usize;
        result_indices[d] = (remaining % dim) as u16;
        remaining /= dim;
    }

    // Scatter into the source's dimension order: ordered[src_dim] =
    // result_indices[result_dim] for each mapped axis (`vm.rs:2395-2402`).
    let mut ordered = vec![0u16; source.dims.len()];
    for (result_dim, mapped) in dim_map.iter().enumerate() {
        if let Some(src_dim) = mapped {
            ordered[*src_dim] = result_indices[result_dim];
        }
    }
    let flat = source.flat_offset_for_indices(&ordered);
    let source = source.clone();
    emit_view_offset_load(&source, flat, ctx, f)
}

/// Lower `ViewSubscriptDynamic { dim_idx }` (Task 4): pop the 1-based runtime
/// index off the wasm stack, bounds-check it against the top view's
/// `dims[dim_idx]`, and fold `(index-1) * strides[dim_idx]` into the view's
/// runtime-offset local; an out-of-bounds index clears the view's validity flag.
/// The *shape* change (dropping `dim_idx`) is compile-time; only the offset
/// addend and validity are runtime. Mirrors `apply_single_subscript_checked`
/// (`bytecode.rs:242`) + `apply_single_subscript` (`bytecode.rs:326`).
fn emit_view_subscript_dynamic(
    dim_idx: usize,
    ctx: &EmitCtx,
    state: &mut EmitState,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    use Instruction as Ins;

    // Read the geometry (stride/bound) before mutating the descriptor's shape.
    let view = view_top(&state.view_stack)?;
    let dim_size = view.dim_at(dim_idx).ok_or_else(|| {
        WasmGenError::Unsupported(format!(
            "wasmgen: ViewSubscriptDynamic dim {dim_idx} out of range"
        ))
    })?;
    let stride = view.stride_at(dim_idx).ok_or_else(|| {
        WasmGenError::Unsupported(format!(
            "wasmgen: ViewSubscriptDynamic dim {dim_idx} out of range"
        ))
    })?;
    // Snapshot the (Copy) runtime-offset/validity locals so the borrow of `view`
    // ends here, freeing `state` for the mutable re-borrow in the allocate path.
    let existing_locals = (view.runtime_off_local, view.valid_local);

    // Lazily allocate (and initialize) the view's runtime-offset + validity
    // locals on its first dynamic subscript: offset 0, valid 1. The two locals
    // are always set together (below), so once one is present so is the other --
    // the `else unreachable!` makes that invariant explicit rather than relying
    // on a bare `.unwrap()` pair.
    let (off_local, valid_local) = match existing_locals {
        (Some(off), Some(valid)) => (off, valid),
        (Some(_), None) | (None, Some(_)) => unreachable!(
            "wasmgen: a dynamically-subscripted view sets runtime_off_local and \
             valid_local together; exactly one was present"
        ),
        (None, None) => {
            let off_local = state.alloc_i32_local();
            let valid_local = state.alloc_i32_local();
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::LocalSet(off_local));
            f.instruction(&Ins::I32Const(1));
            f.instruction(&Ins::LocalSet(valid_local));
            let view = view_top_mut(&mut state.view_stack)?;
            view.runtime_off_local = Some(off_local);
            view.valid_local = Some(valid_local);
            (off_local, valid_local)
        }
    };

    // Park the popped f64 index in the scratch f64 local (free at an opcode
    // boundary) so it can be read twice (bounds check + offset).
    f.instruction(&Ins::LocalSet(ctx.scratch_local));

    // in_bounds = (idx >= 1.0) & (idx <= dim_size). The VM floors the index, but
    // the bound test is on the popped value; using the value directly (>= 1.0,
    // <= dim_size) matches `index_1based == 0 || index_1based > dims[dim_idx]`
    // on the floored u16 for any non-negative index, and a negative index fails
    // `>= 1.0`. valid &= in_bounds (validity is sticky-false, like the VM).
    f.instruction(&Ins::LocalGet(valid_local));
    f.instruction(&Ins::LocalGet(ctx.scratch_local));
    f.instruction(&Ins::F64Floor);
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Ge); // floor(idx) >= 1
    f.instruction(&Ins::LocalGet(ctx.scratch_local));
    f.instruction(&Ins::F64Floor);
    f.instruction(&f64_const(f64::from(dim_size)));
    f.instruction(&Ins::F64Le); // floor(idx) <= dim_size
    f.instruction(&Ins::I32And);
    f.instruction(&Ins::I32And); // valid & in_bounds
    f.instruction(&Ins::LocalSet(valid_local));

    // off_local += (floor(idx) as i32 - 1) * stride. Folded unconditionally: when
    // invalid the read is NaN-gated, so the (possibly bogus) offset is never used.
    f.instruction(&Ins::LocalGet(off_local));
    f.instruction(&Ins::LocalGet(ctx.scratch_local));
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub); // index - 1 (0-based)
    f.instruction(&Ins::I32Const(stride));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(off_local));

    // Drop the subscripted dimension from the compile-time shape.
    let view = view_top_mut(&mut state.view_stack)?;
    view.apply_single_subscript_dynamic(dim_idx)
        .ok_or_else(|| {
            WasmGenError::Unsupported(
                "wasmgen: ViewSubscriptDynamic on a sparse/out-of-range dimension".to_string(),
            )
        })?;
    Ok(())
}

/// Lower `PushSubscriptIndex { bounds }` (Task 4, legacy scalar subscript): pop
/// the 1-based runtime index, range-check it against `bounds`, and accumulate
/// its 0-based value in a fresh i32 local for the eventual `LoadSubscript` fold.
/// An out-of-bounds index clears the accumulation's shared validity flag.
/// Mirrors `vm.rs:1341-1349`.
fn emit_push_subscript_index(bounds: u16, state: &mut EmitState, f: &mut Function) {
    use Instruction as Ins;

    // Allocate the shared validity flag on the first index of an accumulation
    // (init 1 = valid). Subsequent indices reuse it.
    let valid_local = match state.subscript.valid_local {
        Some(v) => v,
        None => {
            let v = state.alloc_i32_local();
            f.instruction(&Ins::I32Const(1));
            f.instruction(&Ins::LocalSet(v));
            state.subscript.valid_local = Some(v);
            v
        }
    };

    // A fresh i32 local holds this index's 0-based value until LoadSubscript
    // folds it (several PushSubscriptIndex precede one LoadSubscript).
    let idx_local = state.alloc_i32_local();

    // idx_i32 = floor(pop) as i32 (the 1-based index).
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::I32TruncSatF64S);
    // Keep a copy for the bounds check (LocalTee leaves it on the stack).
    f.instruction(&Ins::LocalTee(idx_local));

    // in_bounds = (idx >= 1) & (idx <= bounds). The VM's test is
    // `index == 0 || index > bounds` on a u16 (so a 0 or negative index, which
    // `floor as i32` yields <= 0, also fails `>= 1`). valid &= in_bounds.
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32GeS); // idx >= 1
    f.instruction(&Ins::LocalGet(idx_local));
    f.instruction(&Ins::I32Const(i32::from(bounds)));
    f.instruction(&Ins::I32LeS); // idx <= bounds
    f.instruction(&Ins::I32And);
    f.instruction(&Ins::LocalGet(valid_local));
    f.instruction(&Ins::I32And);
    f.instruction(&Ins::LocalSet(valid_local));

    // Store the 0-based index (idx - 1) for the fold.
    f.instruction(&Ins::LocalGet(idx_local));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(idx_local));

    state.subscript.indices.push((idx_local, bounds));
}

/// Lower `LoadSubscript { off }` (Task 4, legacy scalar subscript): fold the
/// accumulated 0-based runtime indices into a row-major flat offset and push
/// `curr[module_off + off + flat]`, or NaN when the accumulation is invalid.
/// Mirrors `vm.rs:1351-1366`: `flat = 0; for (i, b) in indices { flat = flat*b
/// + i }`. Clears the accumulator.
fn emit_load_subscript(off: u16, ctx: &EmitCtx, state: &mut EmitState, f: &mut Function) {
    use Instruction as Ins;
    use wasm_encoder::BlockType;

    let indices = std::mem::take(&mut state.subscript.indices);
    let valid_local = state.subscript.valid_local.take();

    let emit_load = |f: &mut Function| {
        // Dynamic address part = (module_off + flat) * 8, where the row-major
        // fold is `flat = (((i0)*b1 + i1)*b2 + i2)...` (the VM multiplies the
        // running index by each entry's bound then adds the entry's index).
        f.instruction(&Ins::LocalGet(ctx.module_off_local));
        // flat fold:
        if indices.is_empty() {
            f.instruction(&Ins::I32Const(0));
        } else {
            // Start with i0.
            f.instruction(&Ins::LocalGet(indices[0].0));
            for (idx_local, bounds) in &indices[1..] {
                f.instruction(&Ins::I32Const(i32::from(*bounds)));
                f.instruction(&Ins::I32Mul);
                f.instruction(&Ins::LocalGet(*idx_local));
                f.instruction(&Ins::I32Add);
            }
        }
        f.instruction(&Ins::I32Add); // module_off + flat
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul); // (module_off + flat) * 8
        f.instruction(&Ins::F64Load(memarg(slot_byte_offset(ctx.curr_base, off))));
    };

    match valid_local {
        Some(valid_local) => {
            // if valid == 0 { NaN } else { load }
            f.instruction(&Ins::LocalGet(valid_local));
            f.instruction(&Ins::I32Eqz);
            f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
            f.instruction(&f64_const(f64::NAN));
            f.instruction(&Ins::Else);
            emit_load(f);
            f.instruction(&Ins::End);
        }
        // No PushSubscriptIndex preceded this (a 0-dim subscript): always valid.
        None => emit_load(f),
    }
}

/// Emit a store of the f64 already on the wasm stack into the module-relative
/// slot `off` of `chunk_base`. `f64.store` wants `[addr_i32, value_f64]`, but
/// the value is on top, so stash it in the scratch local, push the address,
/// then reload the value.
fn emit_assign(chunk_base: u32, off: u16, ctx: &EmitCtx, f: &mut Function) {
    f.instruction(&Instruction::LocalSet(ctx.scratch_local));
    push_module_relative_base(ctx, f);
    f.instruction(&Instruction::LocalGet(ctx.scratch_local));
    f.instruction(&Instruction::F64Store(memarg(slot_byte_offset(
        chunk_base, off,
    ))));
}

/// Lower a supported binary op. Operands are already on the wasm stack in push
/// order `[l, r]`; the VM pops `r` then `l` and computes `l op r`, so the
/// non-commutative wasm ops (`f64.sub`/`f64.div`) are already correct.
/// Comparisons yield an i32 0/1 which is converted to f64 1.0/0.0 because
/// downstream opcodes consume booleans as f64 (matching `eval_op2`).
fn emit_op2(op: Op2, ctx: &EmitCtx, f: &mut Function) -> Result<(), WasmGenError> {
    match op {
        Op2::Add => {
            f.instruction(&Instruction::F64Add);
        }
        Op2::Sub => {
            f.instruction(&Instruction::F64Sub);
        }
        Op2::Mul => {
            f.instruction(&Instruction::F64Mul);
        }
        Op2::Div => {
            f.instruction(&Instruction::F64Div);
        }
        Op2::Gt => emit_cmp(f, &Instruction::F64Gt),
        Op2::Gte => emit_cmp(f, &Instruction::F64Ge),
        Op2::Lt => emit_cmp(f, &Instruction::F64Lt),
        Op2::Lte => emit_cmp(f, &Instruction::F64Le),
        // `Eq` is `approx_eq(l, r) as f64`: the operands `[l, r]` are already in
        // call order, so `call approx_eq` then widen the i32 1/0 to f64.
        Op2::Eq => {
            emit_call_approx_eq(ctx, f);
            f.instruction(&Instruction::F64ConvertI32U);
        }
        // `And`/`Or` are `(is_truthy(l) OP is_truthy(r)) as f64`.
        Op2::And => emit_logical(ctx, f, Instruction::I32And),
        Op2::Or => emit_logical(ctx, f, Instruction::I32Or),
        // `Exp` is `l.powf(r)`: the operands `[l, r]` are already in call
        // order, so `call pow` directly. Matches `powf` for a positive base
        // (a negative base diverges -- see `super::math::emit_pow`).
        Op2::Exp => {
            f.instruction(&Instruction::Call(ctx.helpers.pow));
        }
        // `Mod` is `l.rem_euclid(r)` (result in [0, |r|)), routed through the
        // `mod_euclid` helper (`[l, r]` already in call order).
        Op2::Mod => {
            f.instruction(&Instruction::Call(ctx.helpers.mod_euclid));
        }
    }
    Ok(())
}

/// Lower `Op2::And`/`Op2::Or`: `(is_truthy(l) OP is_truthy(r)) as f64`, with
/// `combine` the bitwise `i32.and`/`i32.or` that realizes `OP`.
///
/// The operands are on the stack as `[l, r]` (`r` on top), and the wasm operand
/// stack is strict LIFO, so `l` cannot be reduced while `r` sits above it.
/// Park `r` in the scratch f64 local (the same local `emit_assign` uses; it is
/// free here and -- in the `BinOpAssign*` callers -- is overwritten by
/// `emit_assign` before its next read), reduce `is_truthy(l)`, push `r` back and
/// reduce `is_truthy(r)`, then combine. Each `is_truthy` yields an i32 that is
/// exactly 0 or 1, so the bitwise `combine` equals the logical operator; and
/// because `is_truthy` is pure and total, evaluating both operands is
/// bit-identical to the VM's short-circuiting `&&`/`||`.
fn emit_logical(ctx: &EmitCtx, f: &mut Function, combine: Instruction) {
    // stack: [l, r] -> scratch = r; stack: [l]
    f.instruction(&Instruction::LocalSet(ctx.scratch_local));
    // is_truthy(l); stack: [t_l]
    emit_is_truthy(ctx, f);
    // bring r back; is_truthy(r); stack: [t_l, t_r]
    f.instruction(&Instruction::LocalGet(ctx.scratch_local));
    emit_is_truthy(ctx, f);
    // combine and widen to f64 1.0/0.0
    f.instruction(&combine);
    f.instruction(&Instruction::F64ConvertI32U);
}

/// Emit an f64 comparison and convert its i32 result to the f64 0.0/1.0 the
/// VM's `eval_op2` produces for comparisons.
fn emit_cmp(f: &mut Function, cmp: &Instruction) {
    f.instruction(cmp);
    f.instruction(&Instruction::F64ConvertI32U);
}

/// Lower the `Apply { func }` opcode, mirroring the VM's `apply()`
/// (`vm.rs:2938`). The three operands are on the wasm stack in push order
/// `[a, b, c]` (`c` on top, matching the VM popping `c` then `b` then `a`);
/// they are parked in the dedicated `ctx.apply_locals` so each builtin can read
/// them any number of times in any order. The result is left on the stack.
///
/// `time`/`dt` for the time-driven builtins are read from `curr[TIME_OFF]` /
/// `curr[DT_OFF]` (absolute global slots, like `LoadGlobalVar`), matching the
/// VM's `time = curr[TIME_OFF]; dt = curr[DT_OFF]`.
fn emit_apply(func: BuiltinId, ctx: &EmitCtx, f: &mut Function) {
    use Instruction as Ins;
    let [a, b, c] = ctx.apply_locals;

    // Pop the three padded operands. The stack top is `c`, so set c, then b,
    // then a (the VM pops in the same order).
    f.instruction(&Ins::LocalSet(c));
    f.instruction(&Ins::LocalSet(b));
    f.instruction(&Ins::LocalSet(a));

    let get = |f: &mut Function, l: u32| {
        f.instruction(&Ins::LocalGet(l));
    };

    match func {
        // ── Native f64 instructions on `a` ────────────────────────────────
        BuiltinId::Abs => {
            get(f, a);
            f.instruction(&Ins::F64Abs);
        }
        BuiltinId::Sqrt => {
            get(f, a);
            f.instruction(&Ins::F64Sqrt);
        }
        // `Int = a.floor()` -- floor, NOT trunc (the VM's choice; they differ
        // for negative arguments).
        BuiltinId::Int => {
            get(f, a);
            f.instruction(&Ins::F64Floor);
        }
        // `Max`/`Min` use the wasm instructions per AC7.3. These differ from the
        // VM's compare form (`if a>b {a} else {b}`) only on NaN/±0; if a corpus
        // model ever surfaces such a divergence, switch the offending op to the
        // compare-and-select form.
        BuiltinId::Max => {
            get(f, a);
            get(f, b);
            f.instruction(&Ins::F64Max);
        }
        BuiltinId::Min => {
            get(f, a);
            get(f, b);
            f.instruction(&Ins::F64Min);
        }

        // ── Compare/arithmetic composed ───────────────────────────────────
        // `Sign = if a>0 {1} else if a<0 {-1} else {0}`, i.e.
        // `a>0 ? 1 : (a<0 ? -1 : 0)`, via two selects. wasm `select` yields its
        // *deeper* operand when the condition is true, so the outer select is
        // expressed with the inverted test `a<=0` (deeper = inner).
        BuiltinId::Sign => {
            // inner = select(-1.0, 0.0, a < 0)  ->  -1 if a<0 else 0
            f.instruction(&f64_const(-1.0));
            f.instruction(&f64_const(0.0));
            get(f, a);
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Lt);
            f.instruction(&Ins::Select);
            // result = select(inner, 1.0, a <= 0)  ->  inner if a<=0 else 1
            f.instruction(&f64_const(1.0));
            get(f, a);
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Le);
            f.instruction(&Ins::Select);
        }
        // `Quantum = if b==0.0 {a} else {(a/b).trunc()*b}` (exact `==`).
        BuiltinId::Quantum => {
            // select(a, (a/b).trunc()*b, b == 0.0)
            get(f, a);
            // (a/b).trunc() * b
            get(f, a);
            get(f, b);
            f.instruction(&Ins::F64Div);
            f.instruction(&Ins::F64Trunc);
            get(f, b);
            f.instruction(&Ins::F64Mul);
            // cond: b == 0.0
            get(f, b);
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Eq);
            f.instruction(&Ins::Select);
        }
        // `SafeDiv = if b != 0.0 {a/b} else {c}` (exact `!=`, NOT approx).
        BuiltinId::SafeDiv => {
            // select(a/b, c, b != 0.0)
            get(f, a);
            get(f, b);
            f.instruction(&Ins::F64Div);
            get(f, c);
            get(f, b);
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Ne);
            f.instruction(&Ins::Select);
        }
        // `Sshape = b + (c-b)/(1.0 + exp(-4.0*(2.0*a-1.0)))`.
        BuiltinId::Sshape => {
            get(f, b);
            // (c - b)
            get(f, c);
            get(f, b);
            f.instruction(&Ins::F64Sub);
            // denom = 1.0 + exp(-4.0 * (2.0*a - 1.0))
            f.instruction(&f64_const(1.0));
            // exp arg: -4.0 * (2.0*a - 1.0)
            f.instruction(&f64_const(-4.0));
            f.instruction(&f64_const(2.0));
            get(f, a);
            f.instruction(&Ins::F64Mul);
            f.instruction(&f64_const(1.0));
            f.instruction(&Ins::F64Sub);
            f.instruction(&Ins::F64Mul);
            f.instruction(&Ins::Call(ctx.helpers.exp));
            f.instruction(&Ins::F64Add); // 1.0 + exp(..)
            f.instruction(&Ins::F64Div); // (c-b) / denom
            f.instruction(&Ins::F64Add); // b + ..
        }

        // ── Transcendentals on `a` (Task 2 helpers) ───────────────────────
        BuiltinId::Exp => emit_call_unary(ctx.helpers.exp, a, ctx, f),
        BuiltinId::Ln => emit_call_unary(ctx.helpers.ln, a, ctx, f),
        BuiltinId::Log10 => emit_call_unary(ctx.helpers.log10, a, ctx, f),
        BuiltinId::Sin => emit_call_unary(ctx.helpers.sin, a, ctx, f),
        BuiltinId::Cos => emit_call_unary(ctx.helpers.cos, a, ctx, f),
        BuiltinId::Tan => emit_call_unary(ctx.helpers.tan, a, ctx, f),
        BuiltinId::Arcsin => emit_call_unary(ctx.helpers.asin, a, ctx, f),
        BuiltinId::Arccos => emit_call_unary(ctx.helpers.acos, a, ctx, f),
        BuiltinId::Arctan => emit_call_unary(ctx.helpers.atan, a, ctx, f),

        // ── Time-driven ───────────────────────────────────────────────────
        // `Step = step(time, dt, a, b) = if time + dt/2 > b {a} else {0.0}`.
        BuiltinId::Step => {
            // select(a, 0.0, time + dt/2 > b)
            get(f, a);
            f.instruction(&f64_const(0.0));
            // time + dt/2.0
            emit_load_global(ctx, f, TIME_OFF);
            emit_load_global(ctx, f, DT_OFF);
            f.instruction(&f64_const(2.0));
            f.instruction(&Ins::F64Div);
            f.instruction(&Ins::F64Add);
            get(f, b);
            f.instruction(&Ins::F64Gt);
            f.instruction(&Ins::Select);
        }
        // `Ramp = ramp(time, slope=a, start=b, end=Some(c))`:
        //   if time > b { if time >= c { a*(c-b) } else { a*(time-b) } } else 0.
        // The Apply form always supplies an end time, so `end.is_some()` is true.
        BuiltinId::Ramp => {
            // done_value = a * (c - b)
            get(f, a);
            get(f, c);
            get(f, b);
            f.instruction(&Ins::F64Sub);
            f.instruction(&Ins::F64Mul);
            // ramping_value = a * (time - b)
            get(f, a);
            emit_load_global(ctx, f, TIME_OFF);
            get(f, b);
            f.instruction(&Ins::F64Sub);
            f.instruction(&Ins::F64Mul);
            // inner = select(done_value, ramping_value, time >= c)
            emit_load_global(ctx, f, TIME_OFF);
            get(f, c);
            f.instruction(&Ins::F64Ge);
            f.instruction(&Ins::Select);
            // result = select(inner, 0.0, time > b)
            f.instruction(&f64_const(0.0));
            emit_load_global(ctx, f, TIME_OFF);
            get(f, b);
            f.instruction(&Ins::F64Gt);
            f.instruction(&Ins::Select);
        }
        // `Pulse = pulse(time, dt, volume=a, first=b, interval=c)` (helper).
        BuiltinId::Pulse => {
            emit_load_global(ctx, f, TIME_OFF);
            emit_load_global(ctx, f, DT_OFF);
            get(f, a);
            get(f, b);
            get(f, c);
            f.instruction(&Ins::Call(ctx.helpers.pulse));
        }

        // ── Constants ─────────────────────────────────────────────────────
        BuiltinId::Inf => {
            f.instruction(&f64_const(f64::INFINITY));
        }
        BuiltinId::Pi => {
            f.instruction(&f64_const(std::f64::consts::PI));
        }
    }
}

/// Lower the `Lookup { base_gf, table_count, mode }` opcode, mirroring the VM's
/// `Lookup` arm (`vm.rs:1710-1731`). The two operands are on the wasm stack as
/// `[element_offset, index]` (`index` on top, matching the VM popping
/// `lookup_index` then `element_offset`).
///
/// Bounds check: `element_offset < 0.0 || element_offset >= table_count as f64`
/// pushes NaN (the VM's `*table_count as usize as f64` widens the compile-time
/// `u16` count to f64). Otherwise the table index is
/// `base_gf + (element_offset as i32)` (the VM's `as usize` truncation; the
/// bounds check guarantees `0 <= element_offset < table_count`, so
/// `i32.trunc_sat` is exact and non-negative); its `(data_off, count)` is read
/// from the GF directory at `gf_directory_base + table_idx*8`, and the result
/// comes from a static `call` to the mode's helper (the mode is known at
/// compile time). The result is left on the stack.
///
/// `index`/`element_offset` are parked in [`scratch_local`](EmitCtx::scratch_local)
/// and `apply_locals[0]` -- both free f64 scratch locals at an opcode boundary
/// (nothing from a prior opcode is live there; `Lookup` and `Apply` never share
/// a live operand within one opcode). The i32 directory address carries no
/// dedicated local (the opcode-program function reserves none), so it is
/// recomputed for the `count` read; the recompute is a handful of cheap integer
/// ops.
fn emit_lookup(
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    ctx: &EmitCtx,
    f: &mut Function,
) {
    use Instruction as Ins;
    use wasm_encoder::BlockType;

    let index_local = ctx.scratch_local;
    let elem_off_local = ctx.apply_locals[0];

    // Pop the operands. `index` is on top, then `element_offset`.
    f.instruction(&Ins::LocalSet(index_local));
    f.instruction(&Ins::LocalSet(elem_off_local));

    // bounds = (element_offset < 0.0) | (element_offset >= table_count as f64)
    f.instruction(&Ins::LocalGet(elem_off_local));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::LocalGet(elem_off_local));
    f.instruction(&f64_const(table_count as f64));
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    // out of range -> NaN
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Else);

    let helper_idx = match mode {
        LookupMode::Interpolate => ctx.helpers.lookup_interp,
        LookupMode::Forward => ctx.helpers.lookup_forward,
        LookupMode::Backward => ctx.helpers.lookup_backward,
    };

    // data_off = i32.load[dir_addr + 0]; count = i32.load[dir_addr + 4], where
    // dir_addr = gf_directory_base + (base_gf + (element_offset as i32)) * 8.
    push_gf_directory_addr(ctx, f, base_gf, elem_off_local);
    f.instruction(&Ins::I32Load(i32_memarg(0)));
    push_gf_directory_addr(ctx, f, base_gf, elem_off_local);
    f.instruction(&Ins::I32Load(i32_memarg(4)));
    // index, then call the mode's helper -> f64 result.
    f.instruction(&Ins::LocalGet(index_local));
    f.instruction(&Ins::Call(helper_idx));

    f.instruction(&Ins::End); // end if
}

/// Push the byte address of table `base_gf + (element_offset as i32)`'s GF
/// directory entry: `gf_directory_base + (base_gf + elem_off_i32) * 8`.
/// `element_offset` is in `elem_off_local` (f64); `i32.trunc_sat_f64_s` matches
/// the VM's `as usize` for the bounds-checked non-negative offset.
fn push_gf_directory_addr(
    ctx: &EmitCtx,
    f: &mut Function,
    base_gf: GraphicalFunctionId,
    elem_off_local: u32,
) {
    use Instruction as Ins;
    f.instruction(&Ins::I32Const(ctx.gf_directory_base as i32));
    f.instruction(&Ins::I32Const(base_gf as i32));
    f.instruction(&Ins::LocalGet(elem_off_local));
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Add); // table_idx = base_gf + elem_off
    f.instruction(&Ins::I32Const(GF_DIRECTORY_ENTRY_BYTES));
    f.instruction(&Ins::I32Mul); // table_idx * 8
    f.instruction(&Ins::I32Add); // gf_directory_base + table_idx*8
}

/// A 4-byte (i32) memory access with a static byte `offset` (for reading a GF
/// directory entry's two i32 fields). The directory is 8-byte aligned, so a
/// 4-byte access at offset 0 or 4 is naturally aligned.
fn i32_memarg(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2, // log2(4): a 4-byte i32 access
        memory_index: 0,
    }
}

// ============================================================================
// Array view stack + reducers (Phase 5 Tasks 1-2)
// ============================================================================

/// Borrow the top view descriptor, erroring (rather than panicking) on an empty
/// stack -- malformed bytecode rather than a wrong module.
fn view_top(view_stack: &[ViewDesc]) -> Result<&ViewDesc, WasmGenError> {
    view_stack.last().ok_or_else(|| {
        WasmGenError::Unsupported("wasmgen: view opcode on empty view stack".to_string())
    })
}

/// Mutably borrow the top view descriptor for a transform opcode.
fn view_top_mut(view_stack: &mut [ViewDesc]) -> Result<&mut ViewDesc, WasmGenError> {
    view_stack.last_mut().ok_or_else(|| {
        WasmGenError::Unsupported("wasmgen: view transform on empty view stack".to_string())
    })
}

/// Resolve a dim-list id to `(dim sizes, dim ids)` for `PushVarView`/
/// `PushTempView`: each entry is a `DimId`, and the size comes from
/// `ctx.dimensions[DimId].size` (`vm.rs:1745`).
fn resolve_dim_list_dims(
    ctx: &EmitCtx,
    dim_list_id: u16,
) -> Result<(Vec<u16>, Vec<u16>), WasmGenError> {
    let (n_dims, dim_ids) = ctx
        .ctx
        .dim_lists
        .get(dim_list_id as usize)
        .map(|(n, ids)| (*n as usize, *ids))
        .ok_or_else(|| {
            WasmGenError::Unsupported(format!("wasmgen: dim_list_id {dim_list_id} out of range"))
        })?;
    let mut dims = Vec::with_capacity(n_dims);
    for &dim_id in dim_ids.iter().take(n_dims) {
        let size = ctx
            .ctx
            .dimensions
            .get(dim_id as usize)
            .map(|d| d.size)
            .ok_or_else(|| {
                WasmGenError::Unsupported(format!("wasmgen: DimId {dim_id} out of range"))
            })?;
        dims.push(size);
    }
    let dim_id_vec = dim_ids[..n_dims].to_vec();
    Ok((dims, dim_id_vec))
}

/// Resolve a dim-list id to its raw dimension sizes for `PushVarViewDirect`,
/// where each entry is a literal dimension size, not a `DimId` (`vm.rs:1780`).
/// The caller supplies the view's `dim_ids` itself (all zero -- this view is the
/// base for a dynamic subscript, which does not broadcast), so only the sizes
/// are returned here.
fn resolve_dim_list_raw(ctx: &EmitCtx, dim_list_id: u16) -> Result<Vec<u16>, WasmGenError> {
    let (n_dims, sizes) = ctx
        .ctx
        .dim_lists
        .get(dim_list_id as usize)
        .map(|(n, ids)| (*n as usize, *ids))
        .ok_or_else(|| {
            WasmGenError::Unsupported(format!("wasmgen: dim_list_id {dim_list_id} out of range"))
        })?;
    Ok(sizes[..n_dims].to_vec())
}

/// The absolute byte address of temp element `index` of temp `temp_id`:
/// `temp_storage_base + (temp_offsets[temp_id] + index) * 8`.
fn temp_element_byte_addr(ctx: &EmitCtx, temp_id: u8, index: u32) -> Result<u64, WasmGenError> {
    let temp_off = *ctx.ctx.temp_offsets.get(temp_id as usize).ok_or_else(|| {
        WasmGenError::Unsupported(format!("wasmgen: temp id {temp_id} out of range"))
    })? as u64;
    Ok(u64::from(ctx.temp_storage_base) + (temp_off + u64::from(index)) * u64::from(SLOT_SIZE))
}

/// Lower `LoadTempDynamic { temp_id }`: pop a runtime index (the VM does
/// `stack.pop().floor() as usize`), compute the temp element address, and load.
///
/// The address is `temp_storage_base + temp_offsets[temp_id]*8 + index*8`; the
/// constant base/offset ride in the `memarg.offset`, so only `index*8` is
/// computed at runtime. `i32.trunc_sat_f64_s` of `floor(index)` reproduces the
/// VM's `floor() as usize` for a non-negative in-range index.
fn emit_load_temp_dynamic(
    ctx: &EmitCtx,
    temp_id: u8,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    use Instruction as Ins;
    let base = temp_element_byte_addr(ctx, temp_id, 0)?;
    // index (f64, on top) -> floor -> i32 -> *8 (byte stride)
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::F64Load(memarg(base)));
    Ok(())
}

/// Push the f64 value of view element `iter_idx` onto the wasm stack, reading
/// from the byte address [`ViewDesc::element_addr`] computes. This is the single
/// element-read primitive the reducers (Task 2) and -- for static/temp/var
/// views -- the iteration loop (Task 3) build on.
///
/// The constant part of the address rides in the `memarg.offset`; the dynamic
/// part of the wasm address is `module_off * 8` for a module-relative view (0 in
/// the current single-root scope, but emitted for Phase 7 generality) and a bare
/// `0` otherwise. A dynamically-subscripted view (Task 4) returns `Unsupported`
/// here.
///
/// Landed with the view machinery (Task 1) as the single element-read primitive;
/// its first consumer is the array reducer (Task 2), with the iteration loop
/// (Task 3) and Phase 6 to follow.
fn emit_view_element_load(
    desc: &ViewDesc,
    iter_idx: usize,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let addr = desc
        .element_addr(iter_idx, ctx.curr_base, ctx.temp_storage_base, ctx.ctx)
        .ok_or_else(bad_temp_view)?;
    emit_addr_load(addr, ctx, f);
    Ok(())
}

/// Push the f64 value of the view element at an *already-computed* flat slot
/// offset (the broadcast paths -- `LoadIterViewTop` / `LoadBroadcastElement` --
/// build the flat offset themselves rather than from an iteration index).
fn emit_view_offset_load(
    desc: &ViewDesc,
    flat: usize,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let addr = desc
        .element_addr_for_flat(flat, ctx.curr_base, ctx.temp_storage_base, ctx.ctx)
        .ok_or_else(bad_temp_view)?;
    emit_addr_load(addr, ctx, f);
    Ok(())
}

/// Emit the f64 load for a resolved [`ElementAddr`]: the constant part rides in
/// the `memarg.offset`; the dynamic part is `module_off * 8` for a module-
/// relative view plus, for a dynamically-subscripted view (Task 4), the
/// `runtime_off_local * 8` runtime addend (matching the VM's
/// `curr[module_off + base_off + flat + dynamic]`). When the view carries a
/// validity flag (`valid_local`), the whole load is wrapped in a guard that
/// yields NaN when the flag is 0 -- the VM's out-of-bounds-subscript NaN.
fn emit_addr_load(addr: ElementAddr, ctx: &EmitCtx, f: &mut Function) {
    use Instruction as Ins;
    use wasm_encoder::BlockType;

    // Validity gate (dynamic subscript only): `if valid == 0 { NaN } else <load>`.
    if let Some(valid_local) = addr.valid_local {
        f.instruction(&Ins::LocalGet(valid_local));
        f.instruction(&Ins::I32Eqz);
        f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
        f.instruction(&f64_const(f64::NAN));
        f.instruction(&Ins::Else);
        emit_addr_load_unguarded(addr, ctx, f);
        f.instruction(&Ins::End);
    } else {
        emit_addr_load_unguarded(addr, ctx, f);
    }
}

/// The bare load half of [`emit_addr_load`] (no validity guard): push the
/// dynamic address part, then `f64.load` with the constant `memarg.offset`. The
/// dynamic part sums `module_off * 8` (module-relative views) and
/// `runtime_off_local * 8` (a dynamic subscript's accumulated offset); if
/// neither is present it is a bare `0`.
fn emit_addr_load_unguarded(addr: ElementAddr, ctx: &EmitCtx, f: &mut Function) {
    use Instruction as Ins;
    let mut pushed = false;
    if addr.module_relative {
        push_module_relative_base(ctx, f);
        pushed = true;
    }
    if let Some(off_local) = addr.runtime_off_local {
        // runtime_off_local is a slot offset; convert to bytes.
        f.instruction(&Ins::LocalGet(off_local));
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul);
        if pushed {
            f.instruction(&Ins::I32Add);
        }
        pushed = true;
    }
    if !pushed {
        f.instruction(&Ins::I32Const(0));
    }
    f.instruction(&Ins::F64Load(memarg(addr.const_byte_offset)));
}

/// The `Unsupported` error for a temp-backed view whose `base_off` is not a
/// valid temp id (`temp_offsets[base_off]` out of range) -- malformed bytecode
/// rather than a wrong module.
fn bad_temp_view() -> WasmGenError {
    WasmGenError::Unsupported(
        "wasmgen: array element read references an out-of-range temp id".to_string(),
    )
}

/// Lower one array reducer over the top `ViewDesc` (the descriptor stays on the
/// stack; the production pattern is `PushStaticView; Array<Reduce>; PopView`).
///
/// Reproduces `reduce_view` (`vm.rs:2802-2840`) and the per-reducer arms
/// (`vm.rs:2216-2309`) exactly, including the asymmetry:
/// - an **invalid** view (`valid_local` present and 0) yields NaN for *every*
///   reducer, including `ArraySum` (`reduce_view`'s `if !is_valid { NaN }`);
/// - an **empty-but-valid** view (`size() == 0`) yields `0.0` for `ArraySum`,
///   `NaN` for Max/Min/Mean/Stddev, and `0` for `ArraySize`.
///
/// The fold is fully unrolled over the compile-time `size()`: reducer arrays are
/// small, and unrolling reads each element at its compile-time-known address via
/// [`emit_view_element_load`], so no runtime loop or precomputed offset table is
/// needed for the static/temp views the reducer path produces. `ArrayMax`/
/// `ArrayMin` use the VM's compare-and-select form (`if v > acc { v } else
/// { acc }`), not `f64.max`/`f64.min`, matching the reduce path (AC7.3).
fn emit_array_reduce(
    op: &Opcode,
    desc: &ViewDesc,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    use Instruction as Ins;

    // ArraySize is always defined (the size of the view), independent of
    // validity, and needs no element reads. The VM pushes `view.size() as f64`
    // unconditionally (`vm.rs:2306`).
    if matches!(op, Opcode::ArraySize {}) {
        f.instruction(&f64_const(desc.size() as f64));
        return Ok(());
    }

    let size = desc.size();
    let is_sum = matches!(op, Opcode::ArraySum {});

    // The empty-but-valid result, before accounting for an invalid view: 0.0 for
    // Sum, NaN for the others.
    let empty_result = if is_sum { 0.0 } else { f64::NAN };

    if size == 0 {
        // No element reads. For a static view (always valid) this is the final
        // answer; a dynamic view's validity is folded in below.
        f.instruction(&f64_const(empty_result));
    } else {
        emit_reduce_fold(op, desc, size, ctx, f)?;
    }

    // An invalid view (Task 4 dynamic subscript out of bounds) overrides the
    // computed value with NaN for ALL reducers, mirroring `reduce_view`'s
    // leading `if !is_valid { return NaN }`. For static views `valid_local` is
    // `None`, so this is a no-op and the static result stands.
    if let Some(valid_local) = desc.valid_local {
        // Build `select(NaN, computed, valid == 0)`. wasm `select` pops
        // `[a, b, cond]` and yields `a` when `cond != 0`, so `a` must be NaN and
        // `b` the computed value. The computed value is currently on top, so
        // park it (the fold has released `scratch_local` by now), push NaN, push
        // the parked value, then `cond = (valid == 0)`.
        f.instruction(&Ins::LocalSet(ctx.scratch_local));
        f.instruction(&f64_const(f64::NAN)); // a = NaN
        f.instruction(&Ins::LocalGet(ctx.scratch_local)); // b = computed
        f.instruction(&Ins::LocalGet(valid_local));
        f.instruction(&Ins::I32Eqz); // cond = 1 when invalid
        f.instruction(&Ins::Select);
    }

    Ok(())
}

/// Emit the unrolled fold body for a non-empty reducer (size >= 1). Leaves the
/// reduced f64 on the wasm stack. Split out so [`emit_array_reduce`] reads
/// linearly.
fn emit_reduce_fold(
    op: &Opcode,
    desc: &ViewDesc,
    size: usize,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    use Instruction as Ins;
    match op {
        // Sum / Mean / Stddev all begin with the running sum over the elements.
        Opcode::ArraySum {} | Opcode::ArrayMean {} | Opcode::ArrayStddev {} => {
            // sum = e0 + e1 + ... (init 0.0, matching reduce_view's `0.0` init).
            f.instruction(&f64_const(0.0));
            for i in 0..size {
                emit_view_element_load(desc, i, ctx, f)?;
                f.instruction(&Ins::F64Add);
            }
            match op {
                Opcode::ArraySum {} => {}
                Opcode::ArrayMean {} => {
                    // mean = sum / size (size > 0 here).
                    f.instruction(&f64_const(size as f64));
                    f.instruction(&Ins::F64Div);
                }
                Opcode::ArrayStddev {} => {
                    // Two-pass population variance: mean = sum/size (computed
                    // above and on the stack), then variance = mean of
                    // (v - mean)^2, then sqrt. Park the mean so each squared
                    // deviation can reference it.
                    f.instruction(&f64_const(size as f64));
                    f.instruction(&Ins::F64Div);
                    f.instruction(&Ins::LocalSet(ctx.scratch_local)); // scratch = mean
                    // variance_sum = Σ (v - mean)^2
                    f.instruction(&f64_const(0.0));
                    for i in 0..size {
                        emit_view_element_load(desc, i, ctx, f)?;
                        f.instruction(&Ins::LocalGet(ctx.scratch_local));
                        f.instruction(&Ins::F64Sub); // v - mean
                        // (v - mean)^2 via self-multiply. This equals `x * x` on
                        // the host libm and agrees with the VM's `.powf(2.0)`
                        // within floating-point tolerance regardless (`f64::powf`
                        // is libm-dependent, so the two are not guaranteed
                        // bit-identical on every platform).
                        f.instruction(&Ins::LocalTee(ctx.apply_locals[0]));
                        f.instruction(&Ins::LocalGet(ctx.apply_locals[0]));
                        f.instruction(&Ins::F64Mul);
                        f.instruction(&Ins::F64Add);
                    }
                    // stddev = sqrt(variance_sum / size)
                    f.instruction(&f64_const(size as f64));
                    f.instruction(&Ins::F64Div);
                    f.instruction(&Ins::F64Sqrt);
                }
                _ => unreachable!(),
            }
        }
        // Max / Min: fold with the VM's compare-and-select (`if v > acc { v }
        // else { acc }`), init NEG_INFINITY / INFINITY (`vm.rs:2228`/`2245`).
        Opcode::ArrayMax {} | Opcode::ArrayMin {} => {
            let init = if matches!(op, Opcode::ArrayMax {}) {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            f.instruction(&f64_const(init)); // acc
            for i in 0..size {
                // stack: [acc]; load v -> [acc, v]; select(v, acc, cmp).
                emit_view_element_load(desc, i, ctx, f)?;
                // Compute the comparison then select. wasm `select` pops
                // [a, b, cond] and yields a when cond != 0. We want
                // `if v <cmp> acc { v } else { acc }`, so push v then acc and
                // test `v <cmp> acc`. Park acc/v in scratch f64 locals so they
                // can be reused for both the select operands and the compare.
                f.instruction(&Ins::LocalSet(ctx.apply_locals[1])); // b1 = v
                f.instruction(&Ins::LocalSet(ctx.apply_locals[0])); // b0 = acc
                f.instruction(&Ins::LocalGet(ctx.apply_locals[1])); // v   (select arg a)
                f.instruction(&Ins::LocalGet(ctx.apply_locals[0])); // acc (select arg b)
                f.instruction(&Ins::LocalGet(ctx.apply_locals[1])); // v
                f.instruction(&Ins::LocalGet(ctx.apply_locals[0])); // acc
                if matches!(op, Opcode::ArrayMax {}) {
                    f.instruction(&Ins::F64Gt); // v > acc
                } else {
                    f.instruction(&Ins::F64Lt); // v < acc
                }
                f.instruction(&Ins::Select); // v if (cmp) else acc -> new acc
            }
        }
        _ => unreachable!("emit_reduce_fold called with non-reducer opcode"),
    }
    Ok(())
}

/// Push `helper(local)` for a unary `(f64) -> f64` helper: load the f64 local,
/// then `call`.
fn emit_call_unary(helper_idx: u32, src: u32, _ctx: &EmitCtx, f: &mut Function) {
    f.instruction(&Instruction::LocalGet(src));
    f.instruction(&Instruction::Call(helper_idx));
}

/// Push the absolute (module-independent) global slot `off` from `curr`,
/// matching `LoadGlobalVar` (slots 0..4 are reserved globals: TIME/DT/...).
fn emit_load_global(ctx: &EmitCtx, f: &mut Function, off: u16) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::F64Load(memarg(slot_byte_offset(
        ctx.curr_base,
        off,
    ))));
}

/// Lower `LoadPrev { off }`, mirroring the VM (`vm.rs:1320-1328`). A fallback
/// f64 is already on the wasm stack (codegen pushes it immediately before this
/// opcode). Park it in the scratch local, then build `select(fallback,
/// prev_values[module_off+off], use_prev_fallback)`: wasm `select` yields its
/// *deeper* operand when the condition is non-zero, so pushing
/// `[fallback, prev_value, use_prev_fallback]` yields the fallback while the
/// flag is set and the snapshot value once it is cleared.
fn emit_load_prev(off: u16, ctx: &EmitCtx, f: &mut Function) {
    use Instruction as Ins;
    // Park the fallback (top of stack) so the module-relative prev_values
    // address can be pushed beneath it.
    f.instruction(&Ins::LocalSet(ctx.scratch_local));
    f.instruction(&Ins::LocalGet(ctx.scratch_local)); // [fallback]
    // prev_values[module_off + off]
    push_module_relative_base(ctx, f);
    f.instruction(&Ins::F64Load(memarg(slot_byte_offset(
        ctx.prev_values_base,
        off,
    )))); // [fallback, prev_value]
    f.instruction(&Ins::GlobalGet(ctx.use_prev_fallback_global)); // [fallback, prev_value, cond]
    f.instruction(&Ins::Select);
}

/// Lower `LoadInitial { off }`, mirroring the VM (`vm.rs:1332-1340`) with the
/// `part == Initials` branch resolved at compile time from `ctx.step_part`. In
/// the initials program the snapshot is not yet taken, so read
/// `curr[module_off+off]` (the value being computed); in the flows/stocks
/// programs read the post-initials `initial_values[module_off+off]` snapshot.
fn emit_load_initial(off: u16, ctx: &EmitCtx, f: &mut Function) {
    let chunk_base = if ctx.step_part == StepPart::Initials {
        ctx.curr_base
    } else {
        ctx.initial_values_base
    };
    push_module_relative_base(ctx, f);
    f.instruction(&Instruction::F64Load(memarg(slot_byte_offset(
        chunk_base, off,
    ))));
}

/// Name an unsupported opcode without depending on `Debug` (feature-gated via
/// `debug-derive`).
fn unsupported_opcode(op: &Opcode) -> String {
    let name = match op {
        Opcode::PushSubscriptIndex { .. } => "PushSubscriptIndex",
        Opcode::LoadSubscript { .. } => "LoadSubscript",
        Opcode::LoadModuleInput { .. } => "LoadModuleInput",
        Opcode::EvalModule { .. } => "EvalModule",
        Opcode::Apply { .. } => "Apply",
        Opcode::Lookup { .. } => "Lookup",
        // Fused / superinstruction / array opcodes never reach a
        // CompiledSimulation consumer, but name them defensively.
        _ => "opcode",
    };
    format!("wasmgen: unsupported Opcode::{name}")
}

#[cfg(test)]
#[path = "lower_tests.rs"]
mod tests;
