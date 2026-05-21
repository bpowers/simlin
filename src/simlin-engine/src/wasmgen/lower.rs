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

use crate::bytecode::{BuiltinId, ByteCode, Op2, Opcode};

use super::WasmGenError;

/// Bytes per f64 slot.
const SLOT_SIZE: u32 = 8;
/// Alignment exponent for an 8-byte f64 access (log2(8)).
const F64_ALIGN: u32 = 3;

/// Compile-time context for lowering a scalar opcode program over the f64 slab.
///
/// `curr_base`/`next_base` are byte offsets of slot 0 of each chunk within the
/// linear memory. `module_off_local` is the wasm local index holding this
/// instance's `module_off` (the slot base of the module instance within a
/// chunk); the per-program functions take it as their single `i32` parameter.
/// In Phase 1 the root is the only module so `module_off` is always 0, but
/// emitting with it from the start avoids a Phase 7 rewrite.
pub(crate) struct EmitCtx {
    pub curr_base: u32,
    pub next_base: u32,
    /// Byte offset of the GF directory region (8 bytes/entry, indexed by global
    /// table index: `(data_byte_offset: i32, n_points: i32)`). The `Lookup`
    /// opcode reads `directory_base + table_idx*8` to map a table index to its
    /// data location. Both bases are run-invariant: every per-program function
    /// reads the same read-only GF regions.
    // Read by the `Lookup` opcode arm (Task 3); the `allow` is removed there.
    #[allow(dead_code)]
    pub gf_directory_base: u32,
    /// Byte offset of the GF data region (every table's `(x,y)` knots as f64 LE
    /// pairs, concatenated). Retained for completeness/Phase-7 reuse; the
    /// per-table absolute data offset the `Lookup` opcode passes to a helper is
    /// read from the directory, so opcode lowering does not consult this field.
    #[allow(dead_code)]
    pub gf_data_base: u32,
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
/// `cond_depth` condition locals: one scratch f64, then `cond_depth` i32
/// condition locals, then [`APPLY_LOCAL_COUNT`] f64 `Apply` scratch locals.
///
/// Defined once (and consumed by both `module.rs`'s function builders and the
/// `#[cfg(test)]` harness) so the declared local *types and order* match the
/// indices [`apply_locals_for`] hands out. Param 0 is `module_off`.
pub(crate) fn opcode_fn_locals(cond_depth: usize) -> Vec<(u32, ValType)> {
    vec![
        (1, ValType::F64),
        (cond_depth as u32, ValType::I32),
        (APPLY_LOCAL_COUNT, ValType::F64),
    ]
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

/// Lower one opcode program. Value-producing opcodes leave their f64 result on
/// the wasm operand stack; the assignment opcodes emit a store and leave the
/// stack empty, exactly as the VM's stack-machine arms do. `Ret` is a no-op
/// here: the wasm function's terminating `End` is emitted by the caller.
pub(crate) fn emit_bytecode(
    bc: &ByteCode,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // Emit-time stack pointer into `ctx.condition_locals`, mirroring the VM's
    // single `condition` register but generalized to nested `If`s.
    let mut cond_sp: usize = 0;
    for op in &bc.code {
        match op {
            Opcode::LoadConstant { id } => {
                let v = *bc.literals.get(*id as usize).ok_or_else(|| {
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
                let local = *ctx.condition_locals.get(cond_sp).ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: SetCond nesting exceeded reserved condition locals".to_string(),
                    )
                })?;
                // Reduce the f64 condition to i32 truthiness, routing through
                // `approx_eq` so a near-zero / ULP-adjacent condition takes the
                // same branch the VM's `is_truthy(pop)` takes.
                emit_is_truthy(ctx, f);
                f.instruction(&Instruction::LocalSet(local));
                cond_sp += 1;
            }
            Opcode::If {} => {
                if cond_sp == 0 {
                    return Err(WasmGenError::Unsupported(
                        "wasmgen: If without a preceding SetCond".to_string(),
                    ));
                }
                cond_sp -= 1;
                let local = ctx.condition_locals[cond_sp];
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
                let v = *bc.literals.get(*literal_id as usize).ok_or_else(|| {
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
            Opcode::Ret => {
                // The caller emits the function's terminating `End`.
            }
            other => return Err(WasmGenError::Unsupported(unsupported_opcode(other))),
        }
    }
    Ok(())
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

/// Name an unsupported opcode without depending on `Debug` (feature-gated via
/// `debug-derive`).
fn unsupported_opcode(op: &Opcode) -> String {
    let name = match op {
        Opcode::LoadPrev { .. } => "LoadPrev",
        Opcode::LoadInitial { .. } => "LoadInitial",
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
mod tests {
    use super::*;
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, FunctionSection, MemorySection, MemoryType, Module,
        TypeSection, ValType,
    };

    /// Local layout for the test harness function. The function takes
    /// `module_off` as param 0; the scratch f64 and the condition i32(s) are
    /// declared locals.
    const L_MODULE_OFF: u32 = 0;
    const L_SCRATCH: u32 = 1;
    const L_COND_BASE: u32 = 2;

    fn ctx_with_cond_depth(depth: usize) -> EmitCtx {
        EmitCtx {
            curr_base: 0,
            next_base: 4096,
            // The non-Lookup opcode tests place no GF regions; these bases are
            // unused by the opcodes they exercise. The Lookup-opcode tests
            // (which do read these) build their own ctx with real GF bases.
            gf_directory_base: 0,
            gf_data_base: 0,
            dt: 0.5,
            start_time: 1.0,
            final_time: 25.0,
            module_off_local: L_MODULE_OFF,
            scratch_local: L_SCRATCH,
            condition_locals: (0..depth as u32).map(|i| L_COND_BASE + i).collect(),
            apply_locals: apply_locals_for(depth),
            // The helper-function indices are deterministic (helpers occupy the
            // module's first function slots), and `build_module` emits exactly
            // these helper bodies ahead of `eval`, so the indices agree.
            helpers: build_helpers().fns,
        }
    }

    fn bc(literals: Vec<f64>, code: Vec<Opcode>) -> ByteCode {
        ByteCode { literals, code }
    }

    /// Build a module exporting `mem` and an `eval(module_off: i32)` function
    /// whose body is the lowered `bc`. When `with_result`, `eval` returns the
    /// f64 left on the stack. The function declares one scratch f64 local plus
    /// `cond_depth` i32 condition locals.
    ///
    /// Mirrors `module.rs`'s production assembly: the emitted helper functions
    /// ([`build_helpers`]) occupy function indices `0..N` so the `call`s
    /// `emit_bytecode` generates resolve, and `eval` follows at index `N`.
    fn build_module(bc: &ByteCode, ctx: &EmitCtx, with_result: bool, cond_depth: usize) -> Vec<u8> {
        let mut module = Module::new();

        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;

        // Type 0 is `eval`'s signature; each helper's signature follows.
        let mut types = TypeSection::new();
        if with_result {
            types.ty().function([ValType::I32], [ValType::F64]);
        } else {
            types.ty().function([ValType::I32], []);
        }
        for hf in &helpers.functions {
            types.ty().function(hf.params.clone(), hf.results.clone());
        }
        module.section(&types);

        // Function indices follow declaration order: helpers first (0..N), then
        // `eval` at N. Helper type indices are 1..=N (eval's type is 0).
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
        exports.export("eval", ExportKind::Func, n_helpers);
        exports.export("mem", ExportKind::Memory, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in helpers.functions {
            code.function(&hf.body);
        }
        // 1 scratch f64 local, `cond_depth` i32 condition locals, and the 3
        // `Apply` scratch f64 locals -- the same layout production uses.
        let mut func = Function::new(opcode_fn_locals(cond_depth));
        emit_bytecode(bc, ctx, &mut func).expect("lowering should succeed");
        func.instruction(&Instruction::End);
        code.function(&func);
        module.section(&code);

        module.finish()
    }

    /// Emit, validate, instantiate, seed `curr`/`next` slots, run `eval(0)`,
    /// and either return its f64 result (`read_addr == None`) or the f64 at
    /// `read_addr`.
    fn run(
        bc: &ByteCode,
        ctx: &EmitCtx,
        with_result: bool,
        cond_depth: usize,
        seed: &[(u64, f64)],
        read_addr: Option<u64>,
    ) -> f64 {
        let bytes = build_module(bc, ctx, with_result, cond_depth);
        let info = validate(&bytes).expect("emitted module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("emitted module must instantiate")
            .module_addr;

        if !seed.is_empty() {
            let mem = store
                .instance_export(module, "mem")
                .unwrap()
                .as_mem()
                .unwrap();
            store.mem_access_mut_slice(mem, |bytes| {
                for &(addr, v) in seed {
                    let a = addr as usize;
                    bytes[a..a + 8].copy_from_slice(&v.to_le_bytes());
                }
            });
        }

        let eval = store
            .instance_export(module, "eval")
            .unwrap()
            .as_func()
            .unwrap();

        match read_addr {
            None => store
                .invoke_simple_typed(eval, (0_i32,))
                .expect("invocation must succeed"),
            Some(addr) => {
                store
                    .invoke_simple_typed::<(i32,), ()>(eval, (0_i32,))
                    .expect("invocation must succeed");
                let mem = store
                    .instance_export(module, "mem")
                    .unwrap()
                    .as_mem()
                    .unwrap();
                store.mem_access_mut_slice(mem, |bytes| {
                    let a = addr as usize;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
            }
        }
    }

    /// Evaluate a value program (with a 0-depth condition stack) and return its
    /// result.
    fn value(code: Vec<Opcode>, literals: Vec<f64>, seed: &[(u64, f64)]) -> f64 {
        run(
            &bc(literals, code),
            &ctx_with_cond_depth(0),
            true,
            0,
            seed,
            None,
        )
    }

    /// Run an assignment program and read back the stored slot.
    fn stored(code: Vec<Opcode>, literals: Vec<f64>, seed: &[(u64, f64)], read_addr: u64) -> f64 {
        run(
            &bc(literals, code),
            &ctx_with_cond_depth(0),
            false,
            0,
            seed,
            Some(read_addr),
        )
    }

    fn op2(op: Op2) -> Opcode {
        Opcode::Op2 { op }
    }

    // ── LoadConstant ──────────────────────────────────────────────────────

    #[test]
    fn lowers_load_constant() {
        assert_eq!(
            value(vec![Opcode::LoadConstant { id: 0 }], vec![3.5], &[]),
            3.5
        );
    }

    #[test]
    fn lowers_load_constant_selects_right_literal() {
        let code = vec![Opcode::LoadConstant { id: 2 }];
        assert_eq!(value(code, vec![1.0, 2.0, 42.0], &[]), 42.0);
    }

    // ── LoadVar / LoadGlobalVar ───────────────────────────────────────────

    #[test]
    fn lowers_load_var_from_curr() {
        // slot 4 of curr lives at byte 4*8 = 32; module_off is 0.
        let code = vec![Opcode::LoadVar { off: 4 }];
        assert_eq!(value(code, vec![], &[(32, 7.0)]), 7.0);
    }

    #[test]
    fn lowers_load_global_var_absolute() {
        // LoadGlobalVar reads slot `off` ignoring module_off; slot 0 (TIME) at
        // byte 0.
        let code = vec![Opcode::LoadGlobalVar { off: 0 }];
        assert_eq!(value(code, vec![], &[(0, 13.0)]), 13.0);
    }

    #[test]
    fn load_var_honors_module_off() {
        // With a non-zero module_off, LoadVar{off:1} reads curr[module_off+1];
        // LoadGlobalVar{off:1} reads curr[1] regardless. We verify the dynamic
        // base path by running eval with module_off=2 directly.
        let ctx = ctx_with_cond_depth(0);
        let program = bc(vec![], vec![Opcode::LoadVar { off: 1 }]);
        let bytes = build_module(&program, &ctx, true, 0);
        let info = validate(&bytes).expect("module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let mem = store
            .instance_export(module, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        // curr[3] at byte 24 (module_off=2 + off=1).
        store.mem_access_mut_slice(mem, |bytes| {
            bytes[24..32].copy_from_slice(&99.0_f64.to_le_bytes());
        });
        let eval = store
            .instance_export(module, "eval")
            .unwrap()
            .as_func()
            .unwrap();
        let result: f64 = store.invoke_simple_typed(eval, (2_i32,)).expect("invoke");
        assert_eq!(result, 99.0);
    }

    // ── Op2: arithmetic ───────────────────────────────────────────────────

    #[test]
    fn lowers_arithmetic_ops() {
        let lc = |id| Opcode::LoadConstant { id };
        // 2 + 3 = 5
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Add)], vec![2.0, 3.0], &[]),
            5.0
        );
        // 2 - 3 = -1 (operand order: l=2, r=3)
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Sub)], vec![2.0, 3.0], &[]),
            -1.0
        );
        // 2 * 3 = 6
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Mul)], vec![2.0, 3.0], &[]),
            6.0
        );
        // 3 / 2 = 1.5 (operand order: l=3, r=2)
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Div)], vec![3.0, 2.0], &[]),
            1.5
        );
    }

    #[test]
    fn op2_operand_order_matches_vm() {
        // The VM computes `l op r` with l pushed first. births = pop * rate:
        // pop=slot4 (byte 32), constant rate.
        let code = vec![
            Opcode::LoadVar { off: 4 },
            Opcode::LoadConstant { id: 0 },
            op2(Op2::Mul),
        ];
        assert_eq!(value(code, vec![0.1], &[(32, 100.0)]), 10.0);
    }

    // ── Op2: comparisons yield f64 0.0/1.0 ────────────────────────────────

    #[test]
    fn lowers_comparisons_to_f64_bool() {
        let lc = |id| Opcode::LoadConstant { id };
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Gt)], vec![2.0, 1.0], &[]),
            1.0
        );
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Gt)], vec![1.0, 2.0], &[]),
            0.0
        );
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Gte)], vec![1.0, 1.0], &[]),
            1.0
        );
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Lt)], vec![1.0, 2.0], &[]),
            1.0
        );
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Lte)], vec![1.0, 1.0], &[]),
            1.0
        );
    }

    // ── Not ───────────────────────────────────────────────────────────────

    #[test]
    fn lowers_not_truthiness() {
        let lc = |id| Opcode::LoadConstant { id };
        assert_eq!(value(vec![lc(0), Opcode::Not {}], vec![0.0], &[]), 1.0);
        assert_eq!(value(vec![lc(0), Opcode::Not {}], vec![5.0], &[]), 0.0);
    }

    // ── SetCond + If ──────────────────────────────────────────────────────

    /// `if cond then t else f`. Mirrors codegen's emission order: push t, push
    /// f, push cond, SetCond, If. Run with a depth-1 condition stack.
    fn if_program(cond: f64, t: f64, f: f64) -> f64 {
        let code = vec![
            Opcode::LoadConstant { id: 1 }, // t
            Opcode::LoadConstant { id: 2 }, // f
            Opcode::LoadConstant { id: 0 }, // cond
            Opcode::SetCond {},
            Opcode::If {},
        ];
        run(
            &bc(vec![cond, t, f], code),
            &ctx_with_cond_depth(1),
            true,
            1,
            &[],
            None,
        )
    }

    #[test]
    fn lowers_if_selects_true_arm() {
        assert_eq!(if_program(1.0, 10.0, 20.0), 10.0);
    }

    #[test]
    fn lowers_if_selects_false_arm_for_zero() {
        assert_eq!(if_program(0.0, 10.0, 20.0), 20.0);
    }

    #[test]
    fn lowers_if_truthy_nonzero_is_true() {
        // Any non-zero condition is true (matches the VM's is_truthy).
        assert_eq!(if_program(0.5, 10.0, 20.0), 10.0);
        assert_eq!(if_program(-3.0, 10.0, 20.0), 10.0);
    }

    #[test]
    fn lowers_if_with_comparison_condition() {
        // if pop > 50 then 1 else 0, pop in slot 4 (byte 32).
        let code = vec![
            Opcode::LoadConstant { id: 0 }, // t = 1
            Opcode::LoadConstant { id: 1 }, // f = 0
            Opcode::LoadVar { off: 4 },     // pop
            Opcode::LoadConstant { id: 2 }, // 50
            op2(Op2::Gt),
            Opcode::SetCond {},
            Opcode::If {},
        ];
        let run_with = |seed: &[(u64, f64)]| {
            run(
                &bc(vec![1.0, 0.0, 50.0], code.clone()),
                &ctx_with_cond_depth(1),
                true,
                1,
                seed,
                None,
            )
        };
        assert_eq!(run_with(&[(32, 100.0)]), 1.0);
        assert_eq!(run_with(&[(32, 10.0)]), 0.0);
    }

    #[test]
    fn lowers_nested_if() {
        // if (if a then b else c) then d else e.
        // codegen order: push d, push e, then walk the cond which is the inner
        // If (push b, push c, push a, SetCond_inner, If_inner), then
        // SetCond_outer, If_outer. literals: a,b,c,d,e at 0..5.
        let code = vec![
            Opcode::LoadConstant { id: 3 }, // d
            Opcode::LoadConstant { id: 4 }, // e
            Opcode::LoadConstant { id: 1 }, // b
            Opcode::LoadConstant { id: 2 }, // c
            Opcode::LoadConstant { id: 0 }, // a
            Opcode::SetCond {},             // inner
            Opcode::If {},                  // inner -> b or c
            Opcode::SetCond {},             // outer (cond = inner result)
            Opcode::If {},                  // outer -> d or e
        ];
        let eval = |a: f64, b: f64, c: f64, d: f64, e: f64| {
            run(
                &bc(vec![a, b, c, d, e], code.clone()),
                &ctx_with_cond_depth(2),
                true,
                2,
                &[],
                None,
            )
        };
        // a truthy -> inner = b. b truthy -> outer = d.
        assert_eq!(eval(1.0, 1.0, 0.0, 100.0, 200.0), 100.0);
        // a falsey -> inner = c. c falsey -> outer = e.
        assert_eq!(eval(0.0, 1.0, 0.0, 100.0, 200.0), 200.0);
        // a truthy -> inner = b=0 (falsey) -> outer = e.
        assert_eq!(eval(1.0, 0.0, 9.0, 100.0, 200.0), 200.0);
    }

    // ── AssignCurr / AssignNext ───────────────────────────────────────────

    #[test]
    fn lowers_assign_curr_constant() {
        // store 42.0 into curr slot 5 (byte 40), read it back.
        let code = vec![
            Opcode::LoadConstant { id: 0 },
            Opcode::AssignCurr { off: 5 },
        ];
        assert_eq!(stored(code, vec![42.0], &[], 40), 42.0);
    }

    #[test]
    fn lowers_assign_const_curr() {
        // AssignConstCurr is emitted by base codegen for a constant-RHS
        // assignment (e.g. a constant initial or aux): curr[off] = literals[id].
        // Store 7.0 into curr slot 6 (byte 48), read it back.
        let code = vec![Opcode::AssignConstCurr {
            off: 6,
            literal_id: 0,
        }];
        assert_eq!(stored(code, vec![7.0], &[], 48), 7.0);
    }

    #[test]
    fn assign_const_curr_honors_module_off() {
        // With module_off=2, AssignConstCurr{off:1} writes curr[3] (byte 24).
        let ctx = ctx_with_cond_depth(0);
        let program = bc(
            vec![3.5],
            vec![Opcode::AssignConstCurr {
                off: 1,
                literal_id: 0,
            }],
        );
        let bytes = build_module(&program, &ctx, false, 0);
        let info = validate(&bytes).expect("module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let eval = store
            .instance_export(module, "eval")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(i32,), ()>(eval, (2_i32,))
            .expect("invoke");
        let mem = store
            .instance_export(module, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        let v = store.mem_access_mut_slice(mem, |bytes| {
            f64::from_le_bytes(bytes[24..32].try_into().unwrap())
        });
        assert_eq!(v, 3.5);
    }

    #[test]
    fn lowers_bin_op_assign_curr() {
        // BinOpAssignCurr is the peephole fusion of `Op2; AssignCurr`: pops
        // [l, r], computes l op r, stores to curr[off]. Mirrors vm.rs:1457.
        // deaths = pop / 80 -> curr slot 6 (byte 48); pop = slot 4 (byte 32).
        let code = vec![
            Opcode::LoadVar { off: 4 },
            Opcode::LoadConstant { id: 0 },
            Opcode::BinOpAssignCurr {
                op: Op2::Div,
                off: 6,
            },
        ];
        assert_eq!(stored(code, vec![80.0], &[(32, 200.0)], 48), 2.5);
    }

    #[test]
    fn lowers_bin_op_assign_next() {
        // BinOpAssignNext is the peephole fusion of `Op2; AssignNext` (stock
        // integration): pops [l, r], computes l op r, stores to next[off].
        // next[pop] = pop + delta, with delta in curr slot 5.
        // next slot 4 lives at next_base(4096) + 32 = 4128.
        let code = vec![
            Opcode::LoadVar { off: 4 }, // pop
            Opcode::LoadVar { off: 5 }, // delta
            Opcode::BinOpAssignNext {
                op: Op2::Add,
                off: 4,
            },
        ];
        // pop=100, delta=3.75 -> 103.75
        assert_eq!(
            stored(code, vec![], &[(32, 100.0), (40, 3.75)], 4128),
            103.75
        );
    }

    #[test]
    fn bin_op_assign_curr_operand_order_matches_vm() {
        // Non-commutative op: l - r with l pushed first.
        // result = a - b -> curr slot 5 (byte 40); a=slot 3 (24), b=slot 4 (32).
        let code = vec![
            Opcode::LoadVar { off: 3 },
            Opcode::LoadVar { off: 4 },
            Opcode::BinOpAssignCurr {
                op: Op2::Sub,
                off: 5,
            },
        ];
        assert_eq!(stored(code, vec![], &[(24, 10.0), (32, 3.0)], 40), 7.0);
    }

    // Note: every `Op2` variant is supported as of Phase 2 (Mod/Exp landed in
    // Task 3), so there is no longer an unsupported operator to drive the
    // `BinOpAssign*` error-propagation path. The fused-`Mod` form is exercised
    // for correctness by `bin_op_assign_curr_mod_stores_rem_euclid`; the
    // clean-error-on-unsupported-*opcode* path is covered by
    // `unsupported_lookup_returns_error` / `unsupported_array_opcode_returns_error`.

    #[test]
    fn lowers_assign_curr_from_expr() {
        // deaths = pop / 80 -> curr slot 6 (byte 48); pop = slot 4 (byte 32).
        let code = vec![
            Opcode::LoadVar { off: 4 },
            Opcode::LoadConstant { id: 0 },
            op2(Op2::Div),
            Opcode::AssignCurr { off: 6 },
        ];
        assert_eq!(stored(code, vec![80.0], &[(32, 200.0)], 48), 2.5);
    }

    #[test]
    fn lowers_assign_next_euler_update() {
        // next[pop] = pop + (births - deaths) * dt, all read from curr.
        // pop=slot4 (32), births=slot5 (40), deaths=slot6 (48); dt=0.5 literal.
        // next slot 4 lives at next_base(4096) + 32 = 4128.
        let code = vec![
            Opcode::LoadVar { off: 4 },     // pop
            Opcode::LoadVar { off: 5 },     // births
            Opcode::LoadVar { off: 6 },     // deaths
            op2(Op2::Sub),                  // births - deaths
            Opcode::LoadConstant { id: 0 }, // dt
            op2(Op2::Mul),                  // (births - deaths) * dt
            op2(Op2::Add),                  // pop + ...
            Opcode::AssignNext { off: 4 },
        ];
        // pop=100, births=10, deaths=2.5 -> 100 + 7.5*0.5 = 103.75
        let seed = &[(32, 100.0), (40, 10.0), (48, 2.5)];
        assert_eq!(stored(code, vec![0.5], seed, 4128), 103.75);
    }

    #[test]
    fn assign_next_honors_module_off() {
        // With module_off=2, AssignNext{off:0} writes next[2]; next_base=4096,
        // so byte 4096 + 2*8 = 4112.
        let ctx = ctx_with_cond_depth(0);
        let program = bc(
            vec![7.0],
            vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignNext { off: 0 },
            ],
        );
        let bytes = build_module(&program, &ctx, false, 0);
        let info = validate(&bytes).expect("module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let eval = store
            .instance_export(module, "eval")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(i32,), ()>(eval, (2_i32,))
            .expect("invoke");
        let mem = store
            .instance_export(module, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        let v = store.mem_access_mut_slice(mem, |bytes| {
            f64::from_le_bytes(bytes[4112..4120].try_into().unwrap())
        });
        assert_eq!(v, 7.0);
    }

    // ── Ret is a no-op ────────────────────────────────────────────────────

    #[test]
    fn ret_emits_nothing() {
        // A program that loads a constant then Ret leaves just the constant.
        let code = vec![Opcode::LoadConstant { id: 0 }, Opcode::Ret];
        assert_eq!(value(code, vec![5.0], &[]), 5.0);
    }

    // ── AC1.5: raw Op2::Div by zero matches IEEE / the VM ─────────────────

    #[test]
    fn div_by_zero_matches_vm_ieee() {
        let lc = |id| Opcode::LoadConstant { id };
        // x/0 -> +Inf
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Div)], vec![1.0, 0.0], &[]),
            f64::INFINITY
        );
        // -x/0 -> -Inf
        assert_eq!(
            value(vec![lc(0), lc(1), op2(Op2::Div)], vec![-1.0, 0.0], &[]),
            f64::NEG_INFINITY
        );
        // 0/0 -> NaN
        let nan = value(vec![lc(0), lc(1), op2(Op2::Div)], vec![0.0, 0.0], &[]);
        assert!(nan.is_nan());
    }

    // ── AC1.4: unsupported opcodes return a clean error, never a panic ────

    #[test]
    fn op2_eq_lowers_without_error() {
        // Eq is now supported (routed through the approx_eq helper), so lowering
        // must succeed where Phase 1 returned Unsupported. Numeric parity is
        // covered by the dedicated approx_eq / Op2::Eq tests below.
        let mut func = Function::new([]);
        let program = bc(vec![1.0, 2.0], vec![op2(Op2::Eq)]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(result.is_ok(), "Op2::Eq should lower without error");
    }

    #[test]
    fn op2_mod_lowers_without_error() {
        // Mod is now supported (rem_euclid via the mod_euclid helper); lowering
        // must succeed where Phase 1 returned Unsupported.
        let mut func = Function::new([]);
        let program = bc(vec![], vec![op2(Op2::Mod)]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(result.is_ok(), "Op2::Mod should lower without error");
    }

    #[test]
    fn op2_exp_lowers_without_error() {
        // Exp is now supported (powf via the pow helper).
        let mut func = Function::new([]);
        let program = bc(vec![], vec![op2(Op2::Exp)]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(result.is_ok(), "Op2::Exp should lower without error");
    }

    // ── Op2::Exp (pow) / Op2::Mod (rem_euclid) numeric parity ─────────────

    /// Evaluate `l Op2::Exp r` (push l, push r, Op2::Exp) -> f64.
    fn eval_exp(l: f64, r: f64) -> f64 {
        value(
            vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                op2(Op2::Exp),
            ],
            vec![l, r],
            &[],
        )
    }

    #[test]
    fn op2_exp_matches_powf_for_positive_base() {
        // The VM's `eval_op2` Exp is `l.powf(r)`. The wasm `pow` helper matches
        // `powf` for a positive base across integer/fractional/negative
        // exponents; assert within the documented helper tolerance.
        let bases: [f64; 6] = [0.5, 1.0, 2.0, 3.7, 10.0, 100.0];
        let exps: [f64; 9] = [-3.0, -1.5, -1.0, 0.0, 0.5, 1.0, 2.0, 2.5, 7.0];
        for &l in &bases {
            for &r in &exps {
                let want = l.powf(r);
                let got = eval_exp(l, r);
                let abs = (got - want).abs();
                let rel = if want != 0.0 { abs / want.abs() } else { abs };
                assert!(
                    abs <= 1e-9 || rel <= 1e-9,
                    "Exp({l}, {r}): got {got}, want {want} (abs {abs:.3e}, rel {rel:.3e})",
                );
            }
        }
        // x == 1 and y == 0 are the helper's exact short-circuits.
        assert_eq!(eval_exp(1.0, 42.0), 1.0);
        assert_eq!(eval_exp(7.0, 0.0), 1.0);
    }

    /// Evaluate `l Op2::Mod r` (push l, push r, Op2::Mod) -> f64.
    fn eval_mod(l: f64, r: f64) -> f64 {
        value(
            vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                op2(Op2::Mod),
            ],
            vec![l, r],
            &[],
        )
    }

    #[test]
    fn op2_mod_matches_rem_euclid_all_sign_combos() {
        // The VM's `eval_op2` Mod is `l.rem_euclid(r)` (result in [0, |r|)),
        // NOT a truncated remainder. Cover all four sign combinations and
        // non-integer operands.
        let cases: &[(f64, f64)] = &[
            (7.0, 3.0),
            (-7.0, 3.0),
            (7.0, -3.0),
            (-7.0, -3.0),
            (7.5, 2.5),
            (-7.5, 2.5),
            (7.5, -2.5),
            (-7.5, -2.5),
            (5.3, 2.1),
            (-5.3, 2.1),
            (5.3, -2.1),
            (-5.3, -2.1),
            (0.0, 3.0),
            (3.0, 3.0),
            (-3.0, 3.0),
            (2.0, 4.0),
        ];
        for &(l, r) in cases {
            let want = l.rem_euclid(r);
            let got = eval_mod(l, r);
            assert!(
                (got - want).abs() < 1e-12,
                "Mod({l}, {r}): got {got}, want {want}",
            );
            // The euclidean remainder is always in [0, |r|).
            assert!(
                (0.0..r.abs()).contains(&got),
                "Mod({l}, {r}) = {got} not in [0, {})",
                r.abs(),
            );
        }
    }

    #[test]
    fn bin_op_assign_curr_mod_stores_rem_euclid() {
        // The peephole-fused `Op2::Mod; AssignCurr` form must also lower (it was
        // an Unsupported case in Phase 1). -7 mod 3 = 2 -> curr slot 5 (byte 40).
        let code = vec![
            Opcode::LoadConstant { id: 0 },
            Opcode::LoadConstant { id: 1 },
            Opcode::BinOpAssignCurr {
                op: Op2::Mod,
                off: 5,
            },
        ];
        assert_eq!(stored(code, vec![-7.0, 3.0], &[], 40), 2.0);
    }

    #[test]
    fn apply_lowers_without_error() {
        // Apply is supported as of Phase 2 Task 4; lowering must succeed where
        // Phase 1 returned Unsupported. (Numeric parity is covered by the
        // dedicated per-builtin tests below.)
        let mut func = Function::new([]);
        let program = bc(
            vec![],
            vec![Opcode::Apply {
                func: BuiltinId::Abs,
            }],
        );
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(result.is_ok(), "Apply should lower without error");
    }

    #[test]
    fn unsupported_lookup_returns_error() {
        use crate::bytecode::LookupMode;
        let mut func = Function::new([]);
        let program = bc(
            vec![],
            vec![Opcode::Lookup {
                base_gf: 0,
                table_count: 1,
                mode: LookupMode::Interpolate,
            }],
        );
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

    #[test]
    fn unsupported_array_opcode_returns_error() {
        let mut func = Function::new([]);
        let program = bc(vec![], vec![Opcode::ArraySum {}]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

    // ── approx_eq helper (AC7.2, AC1.5) ───────────────────────────────────

    /// Build a module exporting `eq(a: f64, b: f64) -> i32` whose body is just
    /// `local.get a; local.get b; call approx_eq`, directly exercising the
    /// emitted helper in isolation. The helper functions are placed at indices
    /// `0..N` (so the `call` resolves) and `eq` follows at index `N`.
    fn build_approx_eq_module() -> Vec<u8> {
        let mut module = Module::new();

        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;

        // Type 0 is `eq`'s signature (f64, f64) -> i32; helper types follow.
        let mut types = TypeSection::new();
        types
            .ty()
            .function([ValType::F64, ValType::F64], [ValType::I32]);
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

        let mut exports = ExportSection::new();
        exports.export("eq", ExportKind::Func, n_helpers);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in helpers.functions {
            code.function(&hf.body);
        }
        let mut eq = Function::new([]);
        eq.instruction(&Instruction::LocalGet(0));
        eq.instruction(&Instruction::LocalGet(1));
        eq.instruction(&Instruction::Call(helpers.fns.approx_eq));
        eq.instruction(&Instruction::End);
        code.function(&eq);
        module.section(&code);

        module.finish()
    }

    /// Run the emitted `approx_eq` helper on `(a, b)` under the interpreter,
    /// returning its i32 result (1 = approximately equal). Built once per call
    /// (cheap; the sample sizes are small).
    fn run_approx_eq(a: f64, b: f64) -> i32 {
        let bytes = build_approx_eq_module();
        let info = validate(&bytes).expect("approx_eq module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("approx_eq module must instantiate")
            .module_addr;
        let eq = store
            .instance_export(module, "eq")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64, f64), i32>(eq, (a, b))
            .expect("eq invocation must succeed")
    }

    /// Assert the emitted helper agrees with the Rust `crate::float::approx_eq`
    /// oracle for both argument orders (the function is symmetric).
    fn assert_approx_eq_matches_oracle(a: f64, b: f64) {
        let oracle = crate::float::approx_eq(a, b) as i32;
        assert_eq!(
            run_approx_eq(a, b),
            oracle,
            "approx_eq({a:?}, {b:?}) disagreed with oracle {oracle}"
        );
        let oracle_swapped = crate::float::approx_eq(b, a) as i32;
        assert_eq!(
            run_approx_eq(b, a),
            oracle_swapped,
            "approx_eq({b:?}, {a:?}) disagreed with oracle {oracle_swapped}"
        );
    }

    /// Move `x` by `k` ULPs in raw-bit order (the increment the float-cmp ordered
    /// map measures within a sign). For small `|k|` and finite `x` this yields a
    /// value the oracle judges 0..|k| ULPs away.
    fn nudge_ulps(x: f64, k: i64) -> f64 {
        f64::from_bits(((x.to_bits() as i64).wrapping_add(k)) as u64)
    }

    #[test]
    fn approx_eq_matches_oracle_curated() {
        // The exact edge cases the task enumerates.
        let na = crate::float::NA; // finite -2^109 sentinel, NOT NaN.
        let cases: &[(f64, f64)] = &[
            // exact-equal
            (1.0, 1.0),
            (0.0, 0.0),
            (-3.5, -3.5),
            (1e300, 1e300),
            // far apart
            (1.0, 2.0),
            (0.0, 1e100),
            (-1e9, 1e9),
            // 1-4 ULP apart around 1.0
            (1.0, nudge_ulps(1.0, 1)),
            (1.0, nudge_ulps(1.0, 2)),
            (1.0, nudge_ulps(1.0, 3)),
            (1.0, nudge_ulps(1.0, 4)),
            // 5 ULPs apart (just past the threshold) around a larger magnitude
            (1000.0, nudge_ulps(1000.0, 5)),
            (1000.0, nudge_ulps(1000.0, 4)),
            // f64::EPSILON-apart around 1.0 (the absolute-epsilon check)
            (1.0, 1.0 + f64::EPSILON),
            (1.0, 1.0 - f64::EPSILON),
            // around zero (subnormals and tiny values straddling the epsilon)
            (0.0, f64::from_bits(1)),                // smallest subnormal
            (0.0, -f64::from_bits(1)),               // negative smallest subnormal
            (0.0, f64::EPSILON),                     // EPSILON away from zero
            (0.0, 1e-300),                           // tiny normal, within epsilon
            (f64::MIN_POSITIVE, -f64::MIN_POSITIVE), // straddle zero by subnormal step
            // signed zeros
            (0.0, -0.0),
            // NaN cases
            (f64::NAN, f64::NAN),
            (f64::NAN, 1.0),
            (f64::NAN, 0.0),
            // the finite :NA: sentinel
            (na, na),
            (na, 0.0),
            (na, 1.0),
            (na, -(2.0_f64).powi(110)),
            // infinities
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
            (f64::INFINITY, f64::NEG_INFINITY),
            (f64::INFINITY, f64::MAX),
            (f64::NEG_INFINITY, f64::MIN),
        ];
        for &(a, b) in cases {
            assert_approx_eq_matches_oracle(a, b);
        }
    }

    #[test]
    fn approx_eq_matches_oracle_randomized() {
        use rand::prelude::*;
        // Fixed seed: a sampled-but-reproducible parity sweep against the oracle.
        let mut rng = StdRng::seed_from_u64(0xA222_02EE);
        for _ in 0..400 {
            // A diverse magnitude/sign base value.
            let exp = rng.random_range(-308i32..=308);
            let mantissa: f64 = rng.random_range(-1.0..1.0);
            let x = mantissa * 10f64.powi(exp);

            // ULP-adjacent partner (often within the 4-ULP threshold, sometimes
            // just past it), exercising the ULP path on both sides of the gap.
            let k = rng.random_range(-8i64..=8);
            assert_approx_eq_matches_oracle(x, nudge_ulps(x, k));

            // An independent unrelated value (usually far apart -> ULP + epsilon
            // both fail), exercising the false path.
            let exp2 = rng.random_range(-308i32..=308);
            let y: f64 = rng.random_range(-1.0..1.0) * 10f64.powi(exp2);
            assert_approx_eq_matches_oracle(x, y);

            // Near-zero straddling pairs (the epsilon absolute check region).
            let tiny_a = rng.random_range(-1.0..1.0) * f64::EPSILON;
            let tiny_b = rng.random_range(-1.0..1.0) * f64::EPSILON;
            assert_approx_eq_matches_oracle(tiny_a, tiny_b);
        }
    }

    // ── Op2::Eq / And / Or / Not / SetCond+If route through approx_eq ─────

    /// Evaluate `l Op2::Eq r` (push l, push r, Op2::Eq) and return the f64 bool.
    fn eval_eq(l: f64, r: f64) -> f64 {
        let lit = vec![l, r];
        value(
            vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                op2(Op2::Eq),
            ],
            lit,
            &[],
        )
    }

    #[test]
    fn op2_eq_matches_vm_for_ulp_adjacent_operands() {
        // Raw `==` would call these unequal, but the VM's `approx_eq` (and so the
        // wasm) calls them equal: 1 ULP and EPSILON-apart around 1.0.
        let one_ulp = nudge_ulps(1.0, 1);
        assert_eq!(eval_eq(1.0, one_ulp), 1.0);
        assert_eq!(eval_eq(1.0, 1.0 + f64::EPSILON), 1.0);
        // 5 ULPs apart at a larger magnitude: past the threshold -> not equal.
        assert_eq!(eval_eq(1000.0, nudge_ulps(1000.0, 5)), 0.0);
        // Exact and far-apart anchors.
        assert_eq!(eval_eq(2.5, 2.5), 1.0);
        assert_eq!(eval_eq(1.0, 2.0), 0.0);
        // NaN == NaN is true under approx_eq (identical bits -> 0 ULPs).
        assert_eq!(eval_eq(f64::NAN, f64::NAN), 1.0);
        assert_eq!(eval_eq(f64::NAN, 1.0), 0.0);
    }

    #[test]
    fn op2_eq_matches_vm_oracle_over_sample() {
        // The whole-expression Eq lowering must agree with the VM's eval_op2 Eq
        // (= approx_eq as f64) across the curated edge values.
        let na = crate::float::NA;
        let cases: &[(f64, f64)] = &[
            (1.0, nudge_ulps(1.0, 3)),
            (1.0, nudge_ulps(1.0, 4)),
            (1.0, nudge_ulps(1.0, 5)),
            (0.0, -0.0),
            (0.0, f64::EPSILON),
            (na, na),
            (na, 0.0),
            (f64::INFINITY, f64::INFINITY),
            (f64::INFINITY, f64::NEG_INFINITY),
        ];
        for &(l, r) in cases {
            let expected = crate::float::approx_eq(l, r) as i8 as f64;
            assert_eq!(eval_eq(l, r), expected, "Eq({l:?}, {r:?})");
        }
    }

    /// Evaluate `l Op2::And r` / `l Op2::Or r`.
    fn eval_logical(op: Op2, l: f64, r: f64) -> f64 {
        value(
            vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                op2(op),
            ],
            vec![l, r],
            &[],
        )
    }

    /// The VM's truthiness: `is_truthy(n) = !approx_eq(n, 0.0)`.
    fn vm_is_truthy(n: f64) -> bool {
        !crate::float::approx_eq(n, 0.0)
    }

    #[test]
    fn op2_and_or_match_vm_truthiness() {
        // EPSILON is falsy (within epsilon of 0); a small-but-not-epsilon value
        // is truthy. These are exactly where raw `!= 0.0` would diverge from the
        // VM.
        let eps = f64::EPSILON;
        let small = 0.001;
        let operands = [
            0.0,
            -0.0,
            eps,
            -eps,
            small,
            -small,
            1.0,
            f64::NAN,
            f64::INFINITY,
        ];
        for &l in &operands {
            for &r in &operands {
                let and_expected = (vm_is_truthy(l) && vm_is_truthy(r)) as i8 as f64;
                let or_expected = (vm_is_truthy(l) || vm_is_truthy(r)) as i8 as f64;
                assert_eq!(
                    eval_logical(Op2::And, l, r),
                    and_expected,
                    "And({l:?}, {r:?})"
                );
                assert_eq!(eval_logical(Op2::Or, l, r), or_expected, "Or({l:?}, {r:?})");
            }
        }
    }

    #[test]
    fn op2_and_or_operand_order_preserved() {
        // And/Or stash the right operand in the scratch local; verify a
        // non-symmetric truthiness pairing still combines correctly and that the
        // scratch reuse doesn't corrupt a following assignment.
        // (truthy AND falsy) = 0; (truthy OR falsy) = 1.
        assert_eq!(eval_logical(Op2::And, 5.0, 0.0), 0.0);
        assert_eq!(eval_logical(Op2::And, 0.0, 5.0), 0.0);
        assert_eq!(eval_logical(Op2::Or, 5.0, 0.0), 1.0);
        assert_eq!(eval_logical(Op2::Or, 0.0, 5.0), 1.0);
    }

    #[test]
    fn bin_op_assign_and_uses_scratch_safely() {
        // BinOpAssignCurr{And} fuses the And reduction with a store; the And
        // lowering reuses the scratch local, which emit_assign then overwrites.
        // Verify the stored result is correct. (truthy AND truthy) = 1 -> slot 5.
        let code = vec![
            Opcode::LoadConstant { id: 0 },
            Opcode::LoadConstant { id: 1 },
            Opcode::BinOpAssignCurr {
                op: Op2::And,
                off: 5,
            },
        ];
        assert_eq!(stored(code, vec![3.0, 7.0], &[], 40), 1.0);
        // (truthy AND falsy) = 0.
        let code0 = vec![
            Opcode::LoadConstant { id: 0 },
            Opcode::LoadConstant { id: 1 },
            Opcode::BinOpAssignCurr {
                op: Op2::And,
                off: 5,
            },
        ];
        assert_eq!(stored(code0, vec![3.0, 0.0], &[], 40), 0.0);
    }

    #[test]
    fn not_matches_vm_approx_eq_truthiness() {
        // Not(n) = (!is_truthy(n)) as f64 = approx_eq(n, 0.0) as f64.
        // EPSILON is "false" so Not(EPSILON) = 1.0; small-but-not-epsilon is
        // "true" so Not(0.001) = 0.0.
        let operands = [0.0, -0.0, f64::EPSILON, -f64::EPSILON, 0.001, 1.0, f64::NAN];
        for &n in &operands {
            let expected = (!vm_is_truthy(n)) as i8 as f64;
            let got = value(
                vec![Opcode::LoadConstant { id: 0 }, Opcode::Not {}],
                vec![n],
                &[],
            );
            assert_eq!(got, expected, "Not({n:?})");
        }
    }

    #[test]
    fn setcond_if_uses_approx_eq_truthiness() {
        // `if cond then t else f` with the condition routed through approx_eq.
        // EPSILON is falsy -> selects the else arm; 0.001 is truthy -> then arm.
        let if_eval = |cond: f64| {
            let code = vec![
                Opcode::LoadConstant { id: 1 }, // t
                Opcode::LoadConstant { id: 2 }, // f
                Opcode::LoadConstant { id: 0 }, // cond
                Opcode::SetCond {},
                Opcode::If {},
            ];
            run(
                &bc(vec![cond, 10.0, 20.0], code),
                &ctx_with_cond_depth(1),
                true,
                1,
                &[],
                None,
            )
        };
        // Falsy conditions (within epsilon of 0) -> else (20.0).
        assert_eq!(if_eval(0.0), 20.0);
        assert_eq!(if_eval(-0.0), 20.0);
        assert_eq!(if_eval(f64::EPSILON), 20.0);
        assert_eq!(if_eval(-f64::EPSILON), 20.0);
        // Truthy conditions -> then (10.0).
        assert_eq!(if_eval(0.001), 10.0);
        assert_eq!(if_eval(1.0), 10.0);
        assert_eq!(if_eval(f64::NAN), 10.0); // is_truthy(NaN) is true
        assert_eq!(if_eval(f64::INFINITY), 10.0);
    }

    // ── Apply: per-builtin parity with the VM's apply() ───────────────────

    /// Run `Apply{func}` over the three operands `(a, b, c)` with `time`/`dt`
    /// seeded into the reserved global slots (TIME at byte 0, DT at byte 8 of
    /// `curr`). The program pushes a, b, c then `Apply`, so `c` is on top --
    /// matching the VM's pop order.
    fn apply_eval(func: BuiltinId, a: f64, b: f64, c: f64, time: f64, dt: f64) -> f64 {
        let code = vec![
            Opcode::LoadConstant { id: 0 },
            Opcode::LoadConstant { id: 1 },
            Opcode::LoadConstant { id: 2 },
            Opcode::Apply { func },
        ];
        // Seed TIME (slot 0 -> byte 0) and DT (slot 1 -> byte 8) of curr.
        value(code, vec![a, b, c], &[(0, time), (8, dt)])
    }

    /// `step`/`ramp`/`pulse` reproduced verbatim from `vm.rs` so the per-builtin
    /// tests compare the wasm output to the exact formula the VM's `apply()`
    /// uses, not to libm.
    fn vm_step(time: f64, dt: f64, height: f64, step_time: f64) -> f64 {
        if time + dt / 2.0 > step_time {
            height
        } else {
            0.0
        }
    }
    fn vm_ramp(time: f64, slope: f64, start: f64, end: f64) -> f64 {
        if time > start {
            if time >= end {
                slope * (end - start)
            } else {
                slope * (time - start)
            }
        } else {
            0.0
        }
    }
    fn vm_pulse(time: f64, dt: f64, volume: f64, first: f64, interval: f64) -> f64 {
        if time < first {
            return 0.0;
        }
        let mut next = first;
        while time >= next {
            if time < next + dt {
                return volume / dt;
            } else if interval <= 0.0 {
                break;
            } else {
                next += interval;
            }
        }
        0.0
    }

    /// Assert a wasm `Apply` result equals an exact f64 value (for the
    /// non-transcendental builtins, which the wasm reproduces bit-for-bit).
    fn assert_apply_exact(func: BuiltinId, a: f64, b: f64, c: f64, time: f64, dt: f64, want: f64) {
        let got = apply_eval(func, a, b, c, time, dt);
        if want.is_nan() {
            assert!(got.is_nan(), "apply result expected NaN, got {got}");
        } else {
            assert_eq!(got, want, "apply({a},{b},{c},t={time},dt={dt})");
        }
    }

    #[test]
    fn apply_abs_sqrt_int() {
        assert_apply_exact(BuiltinId::Abs, -3.5, 0.0, 0.0, 0.0, 1.0, 3.5);
        assert_apply_exact(BuiltinId::Abs, 3.5, 0.0, 0.0, 0.0, 1.0, 3.5);
        assert_apply_exact(BuiltinId::Sqrt, 16.0, 0.0, 0.0, 0.0, 1.0, 4.0);
        // Int is floor, NOT trunc: floor(-2.5) = -3 (trunc would give -2).
        assert_apply_exact(BuiltinId::Int, -2.5, 0.0, 0.0, 0.0, 1.0, (-2.5f64).floor());
        assert_apply_exact(BuiltinId::Int, 2.9, 0.0, 0.0, 0.0, 1.0, 2.0);
        assert_apply_exact(BuiltinId::Int, -2.9, 0.0, 0.0, 0.0, 1.0, -3.0);
    }

    #[test]
    fn apply_min_max() {
        assert_apply_exact(BuiltinId::Max, 3.0, 7.0, 0.0, 0.0, 1.0, 7.0);
        assert_apply_exact(BuiltinId::Max, 7.0, 3.0, 0.0, 0.0, 1.0, 7.0);
        assert_apply_exact(BuiltinId::Min, 3.0, 7.0, 0.0, 0.0, 1.0, 3.0);
        assert_apply_exact(BuiltinId::Min, 7.0, 3.0, 0.0, 0.0, 1.0, 3.0);
        assert_apply_exact(BuiltinId::Max, -1.0, -5.0, 0.0, 0.0, 1.0, -1.0);
        assert_apply_exact(BuiltinId::Min, -1.0, -5.0, 0.0, 0.0, 1.0, -5.0);
    }

    #[test]
    fn apply_sign() {
        assert_apply_exact(BuiltinId::Sign, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0);
        assert_apply_exact(BuiltinId::Sign, -5.0, 0.0, 0.0, 0.0, 1.0, -1.0);
        // Sign(0) = 0 exactly (the VM's `else` branch).
        assert_apply_exact(BuiltinId::Sign, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        assert_apply_exact(BuiltinId::Sign, -0.0, 0.0, 0.0, 0.0, 1.0, 0.0);
    }

    #[test]
    fn apply_quantum() {
        // q == 0 -> x (exact ==, returns a unchanged).
        assert_apply_exact(BuiltinId::Quantum, 3.7, 0.0, 0.0, 0.0, 1.0, 3.7);
        // q != 0 -> (x/q).trunc() * q.
        assert_apply_exact(
            BuiltinId::Quantum,
            7.0,
            2.0,
            0.0,
            0.0,
            1.0,
            (7.0f64 / 2.0).trunc() * 2.0,
        );
        assert_apply_exact(
            BuiltinId::Quantum,
            -7.0,
            2.0,
            0.0,
            0.0,
            1.0,
            (-7.0f64 / 2.0).trunc() * 2.0,
        );
        assert_apply_exact(
            BuiltinId::Quantum,
            5.5,
            0.5,
            0.0,
            0.0,
            1.0,
            (5.5f64 / 0.5).trunc() * 0.5,
        );
    }

    #[test]
    fn apply_safe_div() {
        // b != 0 -> a/b.
        assert_apply_exact(BuiltinId::SafeDiv, 6.0, 3.0, 99.0, 0.0, 1.0, 2.0);
        // b == 0 -> c (the default), via exact `!= 0.0`.
        assert_apply_exact(BuiltinId::SafeDiv, 6.0, 0.0, 99.0, 0.0, 1.0, 99.0);
        // A subnormal (non-zero) denominator still divides, NOT falls back.
        let sub = f64::from_bits(1);
        assert_apply_exact(BuiltinId::SafeDiv, 6.0, sub, 99.0, 0.0, 1.0, 6.0 / sub);
        // -0.0 is == 0.0, so it falls back to c (matches the VM's `b != 0.0`).
        assert_apply_exact(BuiltinId::SafeDiv, 6.0, -0.0, 99.0, 0.0, 1.0, 99.0);
    }

    #[test]
    fn apply_sshape() {
        // b + (c-b)/(1 + exp(-4*(2a-1))), within the exp helper's tolerance.
        for &a in &[0.0f64, 0.25, 0.5, 0.75, 1.0] {
            let want = 2.0 + (8.0 - 2.0) / (1.0 + (-4.0 * (2.0 * a - 1.0)).exp());
            let got = apply_eval(BuiltinId::Sshape, a, 2.0, 8.0, 0.0, 1.0);
            assert!(
                (got - want).abs() < 1e-9,
                "Sshape({a}): got {got}, want {want}",
            );
        }
    }

    #[test]
    fn apply_transcendentals_match_libm() {
        // Each transcendental Apply arm calls the Task 2 helper on `a`; assert
        // it lands within the helpers' documented tolerance of Rust f64.
        let close = |func: BuiltinId, a: f64, want: f64| {
            let got = apply_eval(func, a, 0.0, 0.0, 0.0, 1.0);
            assert!(
                (got - want).abs() < 1e-8 || (got - want).abs() / want.abs() < 1e-8,
                "{func:?}({a}): got {got}, want {want}",
            );
        };
        close(BuiltinId::Exp, 1.5, 1.5f64.exp());
        close(BuiltinId::Ln, 7.0, 7.0f64.ln());
        close(BuiltinId::Log10, 1000.0, 3.0);
        close(BuiltinId::Sin, 0.7, 0.7f64.sin());
        close(BuiltinId::Cos, 0.7, 0.7f64.cos());
        close(BuiltinId::Tan, 0.7, 0.7f64.tan());
        close(BuiltinId::Arcsin, 0.5, 0.5f64.asin());
        close(BuiltinId::Arccos, 0.5, 0.5f64.acos());
        close(BuiltinId::Arctan, 2.0, 2.0f64.atan());
    }

    #[test]
    fn apply_step_across_breakpoint() {
        // step(time, dt, height=a, step_time=b) = if time+dt/2 > b {a} else 0.
        let dt = 0.5;
        for &t in &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0] {
            let want = vm_step(t, dt, 10.0, 3.0);
            assert_apply_exact(BuiltinId::Step, 10.0, 3.0, 0.0, t, dt, want);
        }
    }

    #[test]
    fn apply_ramp_across_breakpoints() {
        // ramp(time, slope=a, start=b, end=c) over its three regimes.
        for &t in &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0] {
            let want = vm_ramp(t, 2.0, 2.0, 5.0);
            assert_apply_exact(BuiltinId::Ramp, 2.0, 2.0, 5.0, t, 1.0, want);
        }
    }

    #[test]
    fn apply_pulse_across_intervals() {
        // pulse(time, dt, volume=a, first=b, interval=c) across several periods,
        // including the no-repeat (interval == 0) case.
        let dt = 1.0;
        for &t in &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] {
            // Repeating pulse: volume 4, first at t=2, every 3.
            assert_apply_exact(
                BuiltinId::Pulse,
                4.0,
                2.0,
                3.0,
                t,
                dt,
                vm_pulse(t, dt, 4.0, 2.0, 3.0),
            );
            // Single pulse: interval 0 -> fires once at t in [first, first+dt).
            assert_apply_exact(
                BuiltinId::Pulse,
                4.0,
                2.0,
                0.0,
                t,
                dt,
                vm_pulse(t, dt, 4.0, 2.0, 0.0),
            );
        }
    }

    #[test]
    fn apply_inf_pi() {
        assert_apply_exact(BuiltinId::Inf, 0.0, 0.0, 0.0, 0.0, 1.0, f64::INFINITY);
        assert_apply_exact(BuiltinId::Pi, 0.0, 0.0, 0.0, 0.0, 1.0, std::f64::consts::PI);
    }

    #[test]
    fn apply_inside_if_does_not_clobber_condition() {
        // An `Apply` in an If arm shares the function with the condition local;
        // the dedicated apply locals must not collide. Build (codegen-padded
        // Apply operands): `if cond then ABS(a) else f`, cond truthy.
        let padded = vec![
            Opcode::LoadConstant { id: 1 }, // a = -4 (the `then` operand)
            Opcode::LoadConstant { id: 3 }, // pad b = 0
            Opcode::LoadConstant { id: 3 }, // pad c = 0
            Opcode::Apply {
                func: BuiltinId::Abs,
            }, // ABS(-4) = 4 -> the `then` value
            Opcode::LoadConstant { id: 2 }, // f = 99
            Opcode::LoadConstant { id: 0 }, // cond = 1 (truthy)
            Opcode::SetCond {},
            Opcode::If {},
        ];
        let got = run(
            &bc(vec![1.0, -4.0, 99.0, 0.0], padded),
            &ctx_with_cond_depth(1),
            true,
            1,
            &[],
            None,
        );
        assert_eq!(got, 4.0, "Apply in an If-then arm should yield ABS(-4)=4");
    }

    // ── max_condition_depth ───────────────────────────────────────────────

    #[test]
    fn max_condition_depth_counts_nesting() {
        // Single If: depth 1.
        let single = bc(vec![], vec![Opcode::SetCond {}, Opcode::If {}]);
        assert_eq!(max_condition_depth(&single), 1);

        // Two sequential Ifs: still depth 1 (LIFO, fully popped between).
        let sequential = bc(
            vec![],
            vec![
                Opcode::SetCond {},
                Opcode::If {},
                Opcode::SetCond {},
                Opcode::If {},
            ],
        );
        assert_eq!(max_condition_depth(&sequential), 1);

        // Interleaved: SetCond, SetCond, If, If -> depth 2. Current codegen
        // never emits this (it walks a condition to completion before its
        // SetCond, so nested IFs come out sequentially); this guards the
        // defensive stack-sizing against a future interleaved emission.
        let nested = bc(
            vec![],
            vec![
                Opcode::SetCond {},
                Opcode::SetCond {},
                Opcode::If {},
                Opcode::If {},
            ],
        );
        assert_eq!(max_condition_depth(&nested), 2);

        // No conditions: depth 0.
        let none = bc(vec![], vec![Opcode::LoadConstant { id: 0 }]);
        assert_eq!(max_condition_depth(&none), 0);
    }
}
