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
//! Anything outside the supported scalar core -- an array/module/lookup opcode,
//! a not-yet-supported `Op2` (Mod/Exp, deferred to Phase 2 Tasks 2-3), or a
//! late-fusion superinstruction that somehow appeared -- returns
//! `WasmGenError::Unsupported` rather than emitting a wrong module.
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

use crate::bytecode::{ByteCode, Op2, Opcode};

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
    // dt/start_time/final_time are the run-invariant time globals. Phase 1
    // reads them from memory via `LoadGlobalVar` (slots 0..4), so they are not
    // consulted here yet; Phase 2 lowers the `TimeStep`/`StartTime`/`FinalTime`
    // builtins to compile-time constants from these, at which point they become
    // live.
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
    /// Function indices of the module's emitted helper functions, so
    /// value-producing opcodes that need the VM's `approx_eq`/transcendental
    /// semantics can `call` them. The same registry is shared by every
    /// per-program function in a module.
    pub helpers: HelperFns,
}

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

    let approx_eq = functions.len() as u32;
    functions.push(HelperFn {
        params: vec![ValType::F64, ValType::F64],
        results: vec![ValType::I32],
        body: emit_approx_eq(),
    });

    BuiltHelpers {
        fns: HelperFns { approx_eq },
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
        // Mod (rem_euclid) and Exp (powf) need runtime helpers landing in
        // Phase 2 Tasks 2-3.
        Op2::Mod | Op2::Exp => {
            return Err(WasmGenError::Unsupported(format!(
                "wasmgen: unsupported binary op {}",
                op2_name(op)
            )));
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

fn op2_name(op: Op2) -> &'static str {
    match op {
        Op2::Add => "Add",
        Op2::Sub => "Sub",
        Op2::Exp => "Exp",
        Op2::Mul => "Mul",
        Op2::Div => "Div",
        Op2::Mod => "Mod",
        Op2::Gt => "Gt",
        Op2::Gte => "Gte",
        Op2::Lt => "Lt",
        Op2::Lte => "Lte",
        Op2::Eq => "Eq",
        Op2::And => "And",
        Op2::Or => "Or",
    }
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
            dt: 0.5,
            start_time: 1.0,
            final_time: 25.0,
            module_off_local: L_MODULE_OFF,
            scratch_local: L_SCRATCH,
            condition_locals: (0..depth as u32).map(|i| L_COND_BASE + i).collect(),
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
        // 1 scratch f64 local, then `cond_depth` i32 condition locals.
        let mut func = Function::new([(1, ValType::F64), (cond_depth as u32, ValType::I32)]);
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

    #[test]
    fn bin_op_assign_with_unsupported_op_returns_error() {
        // A fused unsupported op (e.g. Mod) must still error cleanly.
        let mut func = Function::new([]);
        let program = bc(
            vec![],
            vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Mod,
                    off: 2,
                },
            ],
        );
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

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
    fn unsupported_op2_mod_returns_error() {
        let mut func = Function::new([]);
        let program = bc(vec![], vec![op2(Op2::Mod)]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

    #[test]
    fn unsupported_op2_exp_returns_error() {
        let mut func = Function::new([]);
        let program = bc(vec![], vec![op2(Op2::Exp)]);
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

    #[test]
    fn unsupported_apply_returns_error() {
        use crate::bytecode::BuiltinId;
        let mut func = Function::new([]);
        let program = bc(
            vec![],
            vec![Opcode::Apply {
                func: BuiltinId::Abs,
            }],
        );
        let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
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
