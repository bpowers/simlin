// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Lowering of the scalar `compiler::expr::Expr` IR to WebAssembly instructions.
//!
//! The runtime data model mirrors the bytecode VM: all variable values live in
//! one flat f64 "slab" in linear memory, addressed by slot offset. A model runs
//! over two chunks at a time -- `curr` (the values at the current timestep) and
//! `next` (the values being computed for the following timestep). `Var` reads
//! from `curr`; `AssignCurr`/`AssignNext` store into `curr`/`next`.
//!
//! `dt`, the start time, and the final time never change during a run, so the
//! VM's `LoadGlobalVar` reads of those are lowered to compile-time `f64.const`s
//! here. Only `Time` (which advances each step) is read from a memory slot.

use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

use crate::ast::BinaryOp;
use crate::builtins::BuiltinFn;
use crate::compiler::Expr;
use crate::compiler::dimensions::UnaryOp;

use super::WasmGenError;

/// Slot of the simulation time within a chunk. Mirrors `crate::vm::TIME_OFF`;
/// the other reserved globals (dt/initial/final) are lowered as constants and
/// so are not read from memory here.
const TIME_OFF: usize = 0;

/// Bytes per f64 slot.
const SLOT_SIZE: u32 = 8;
/// Alignment exponent for an 8-byte f64 access (log2(8)).
const F64_ALIGN: u32 = 3;

/// Compile-time context for lowering scalar expressions over the f64 slab.
/// `curr_base`/`next_base` are byte offsets of slot 0 of each chunk.
pub(crate) struct EmitCtx {
    pub curr_base: u32,
    pub next_base: u32,
    pub dt: f64,
    pub start_time: f64,
    pub final_time: f64,
}

impl EmitCtx {
    fn curr_addr(&self, off: usize) -> u64 {
        u64::from(self.curr_base + off as u32 * SLOT_SIZE)
    }

    fn next_addr(&self, off: usize) -> u64 {
        u64::from(self.next_base + off as u32 * SLOT_SIZE)
    }
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

/// Lower an expression. Value expressions leave their f64 result on the wasm
/// operand stack; the `Assign*` forms emit a store and leave the stack empty.
pub(crate) fn lower_expr(expr: &Expr, ctx: &EmitCtx, f: &mut Function) -> Result<(), WasmGenError> {
    match expr {
        Expr::Const(v, _) => {
            f.instruction(&f64_const(*v));
        }
        Expr::Var(off, _) => {
            load_slot(ctx.curr_addr(*off), f);
        }
        Expr::Dt(_) => {
            f.instruction(&f64_const(ctx.dt));
        }
        Expr::Op2(op, lhs, rhs, _) => {
            lower_expr(lhs.as_ref(), ctx, f)?;
            lower_expr(rhs.as_ref(), ctx, f)?;
            lower_binop(*op, f)?;
        }
        Expr::Op1(op, arg, _) => {
            lower_expr(arg.as_ref(), ctx, f)?;
            lower_unop(*op, f)?;
        }
        Expr::If(cond, then_, else_, _) => {
            lower_truthy(cond.as_ref(), ctx, f)?;
            f.instruction(&Instruction::If(BlockType::Result(ValType::F64)));
            lower_expr(then_.as_ref(), ctx, f)?;
            f.instruction(&Instruction::Else);
            lower_expr(else_.as_ref(), ctx, f)?;
            f.instruction(&Instruction::End);
        }
        Expr::AssignCurr(off, rhs) => {
            f.instruction(&Instruction::I32Const(0));
            lower_expr(rhs.as_ref(), ctx, f)?;
            f.instruction(&Instruction::F64Store(memarg(ctx.curr_addr(*off))));
        }
        Expr::AssignNext(off, rhs) => {
            f.instruction(&Instruction::I32Const(0));
            lower_expr(rhs.as_ref(), ctx, f)?;
            f.instruction(&Instruction::F64Store(memarg(ctx.next_addr(*off))));
        }
        Expr::App(builtin, _) => {
            lower_builtin(builtin, ctx, f)?;
        }
        other => return Err(WasmGenError::Unsupported(unsupported_expr(other))),
    }
    Ok(())
}

/// Push `addr`'s f64 (memory base 0 plus a constant memarg offset).
fn load_slot(addr: u64, f: &mut Function) {
    f.instruction(&Instruction::I32Const(0));
    f.instruction(&Instruction::F64Load(memarg(addr)));
}

/// Lower `cond` and reduce it to an i32 boolean (`cond != 0.0`) for `if`.
/// Mirrors the VM's `is_truthy`: any non-zero value is true.
fn lower_truthy(cond: &Expr, ctx: &EmitCtx, f: &mut Function) -> Result<(), WasmGenError> {
    lower_expr(cond, ctx, f)?;
    f.instruction(&f64_const(0.0));
    f.instruction(&Instruction::F64Ne);
    Ok(())
}

fn lower_binop(op: BinaryOp, f: &mut Function) -> Result<(), WasmGenError> {
    match op {
        BinaryOp::Add => {
            f.instruction(&Instruction::F64Add);
        }
        BinaryOp::Sub => {
            f.instruction(&Instruction::F64Sub);
        }
        BinaryOp::Mul => {
            f.instruction(&Instruction::F64Mul);
        }
        BinaryOp::Div => {
            f.instruction(&Instruction::F64Div);
        }
        // Comparisons yield an i32 0/1; the VM represents booleans as f64
        // 1.0/0.0, so convert. (Eq/Neq use exact equality here; the VM uses a
        // ULP-based approx_eq -- a known POC fidelity gap to revisit.)
        BinaryOp::Gt => emit_cmp(f, &Instruction::F64Gt),
        BinaryOp::Gte => emit_cmp(f, &Instruction::F64Ge),
        BinaryOp::Lt => emit_cmp(f, &Instruction::F64Lt),
        BinaryOp::Lte => emit_cmp(f, &Instruction::F64Le),
        BinaryOp::Eq => emit_cmp(f, &Instruction::F64Eq),
        BinaryOp::Neq => emit_cmp(f, &Instruction::F64Ne),
        // Exp (powf) and Mod (rem_euclid) need runtime helpers; And/Or need
        // truthiness reduction of operands already on the stack. Deferred.
        BinaryOp::Exp | BinaryOp::Mod | BinaryOp::And | BinaryOp::Or => {
            return Err(WasmGenError::Unsupported(format!(
                "wasmgen: unsupported binary op {}",
                binop_name(op)
            )));
        }
    }
    Ok(())
}

fn lower_unop(op: UnaryOp, f: &mut Function) -> Result<(), WasmGenError> {
    // By the time an expression reaches this IR, `compiler::dimensions::UnaryOp`
    // carries only `Not` and `Transpose`; unary plus/minus were folded or
    // rewritten into arithmetic earlier in lowering.
    match op {
        UnaryOp::Not => {
            // logical negation of truthiness: (x == 0.0) as f64
            f.instruction(&f64_const(0.0));
            f.instruction(&Instruction::F64Eq);
            f.instruction(&Instruction::F64ConvertI32U);
        }
        UnaryOp::Transpose => {
            return Err(WasmGenError::Unsupported(
                "wasmgen: unsupported unary op Transpose".to_string(),
            ));
        }
    }
    Ok(())
}

fn lower_builtin(
    builtin: &BuiltinFn<Expr>,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    match builtin {
        BuiltinFn::Time => load_slot(ctx.curr_addr(TIME_OFF), f),
        BuiltinFn::TimeStep => {
            f.instruction(&f64_const(ctx.dt));
        }
        BuiltinFn::StartTime => {
            f.instruction(&f64_const(ctx.start_time));
        }
        BuiltinFn::FinalTime => {
            f.instruction(&f64_const(ctx.final_time));
        }
        BuiltinFn::Inf => {
            f.instruction(&f64_const(f64::INFINITY));
        }
        BuiltinFn::Pi => {
            f.instruction(&f64_const(std::f64::consts::PI));
        }
        BuiltinFn::Abs(arg) => {
            lower_expr(arg.as_ref(), ctx, f)?;
            f.instruction(&Instruction::F64Abs);
        }
        BuiltinFn::Sqrt(arg) => {
            lower_expr(arg.as_ref(), ctx, f)?;
            f.instruction(&Instruction::F64Sqrt);
        }
        _ => {
            return Err(WasmGenError::Unsupported(
                "wasmgen: unsupported builtin".to_string(),
            ));
        }
    }
    Ok(())
}

/// Emit an f64 comparison and convert its i32 result to the f64 0.0/1.0 the
/// rest of the lowering expects.
fn emit_cmp(f: &mut Function, cmp: &Instruction) {
    f.instruction(cmp);
    f.instruction(&Instruction::F64ConvertI32U);
}

fn binop_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "Add",
        BinaryOp::Sub => "Sub",
        BinaryOp::Exp => "Exp",
        BinaryOp::Mul => "Mul",
        BinaryOp::Div => "Div",
        BinaryOp::Mod => "Mod",
        BinaryOp::Gt => "Gt",
        BinaryOp::Lt => "Lt",
        BinaryOp::Gte => "Gte",
        BinaryOp::Lte => "Lte",
        BinaryOp::Eq => "Eq",
        BinaryOp::Neq => "Neq",
        BinaryOp::And => "And",
        BinaryOp::Or => "Or",
    }
}

/// Name an unsupported expression variant without depending on `Debug` (which
/// is feature-gated via `debug-derive`).
fn unsupported_expr(expr: &Expr) -> String {
    let name = match expr {
        Expr::Subscript(..) => "Subscript",
        Expr::StaticSubscript(..) => "StaticSubscript",
        Expr::TempArray(..) => "TempArray",
        Expr::TempArrayElement(..) => "TempArrayElement",
        Expr::EvalModule(..) => "EvalModule",
        Expr::ModuleInput(..) => "ModuleInput",
        Expr::AssignTemp(..) => "AssignTemp",
        _ => "expr",
    };
    format!("wasmgen: unsupported Expr::{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Loc;
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, FunctionSection, MemorySection, MemoryType, Module,
        TypeSection,
    };

    fn ctx() -> EmitCtx {
        EmitCtx {
            curr_base: 0,
            next_base: 4096,
            dt: 0.5,
            start_time: 1.0,
            final_time: 25.0,
        }
    }

    fn b(e: Expr) -> Box<Expr> {
        Box::new(e)
    }

    fn konst(v: f64) -> Expr {
        Expr::Const(v, Loc::default())
    }

    /// Build a module exporting `mem` and an `eval` function whose body is
    /// `expr`. When `with_result`, `eval` returns the f64 left on the stack.
    fn build_module(expr: &Expr, ctx: &EmitCtx, with_result: bool) -> Vec<u8> {
        let mut module = Module::new();

        let mut types = TypeSection::new();
        if with_result {
            types.ty().function([], [ValType::F64]);
        } else {
            types.ty().function([], []);
        }
        module.section(&types);

        let mut functions = FunctionSection::new();
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
        exports.export("eval", ExportKind::Func, 0);
        exports.export("mem", ExportKind::Memory, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        let mut func = Function::new([]);
        lower_expr(expr, ctx, &mut func).expect("lowering should succeed");
        func.instruction(&Instruction::End);
        code.function(&func);
        module.section(&code);

        module.finish()
    }

    /// Emit, validate, instantiate, seed `curr`/`next` slots, run, and either
    /// return `eval`'s result (`read_addr == None`) or the f64 at `read_addr`.
    fn run(
        expr: &Expr,
        ctx: &EmitCtx,
        with_result: bool,
        seed: &[(u64, f64)],
        read_addr: Option<u64>,
    ) -> f64 {
        let bytes = build_module(expr, ctx, with_result);
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
                .invoke_simple_typed(eval, ())
                .expect("invocation must succeed"),
            Some(addr) => {
                store
                    .invoke_simple_typed::<(), ()>(eval, ())
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

    /// Evaluate a value expression and return its result.
    fn value(expr: &Expr, seed: &[(u64, f64)]) -> f64 {
        run(expr, &ctx(), true, seed, None)
    }

    /// Run an assignment expression and read back the stored slot.
    fn stored(expr: &Expr, seed: &[(u64, f64)], read_addr: u64) -> f64 {
        run(expr, &ctx(), false, seed, Some(read_addr))
    }

    #[test]
    fn lowers_const() {
        assert_eq!(value(&konst(3.5), &[]), 3.5);
    }

    #[test]
    fn lowers_dt_as_constant() {
        assert_eq!(value(&Expr::Dt(Loc::default()), &[]), 0.5);
    }

    #[test]
    fn lowers_var_load_from_curr() {
        // slot 4 of curr lives at byte offset 4*8 = 32
        assert_eq!(value(&Expr::Var(4, Loc::default()), &[(32, 7.0)]), 7.0);
    }

    #[test]
    fn lowers_arithmetic_ops() {
        let add = Expr::Op2(BinaryOp::Add, b(konst(2.0)), b(konst(3.0)), Loc::default());
        let sub = Expr::Op2(BinaryOp::Sub, b(konst(2.0)), b(konst(3.0)), Loc::default());
        let mul = Expr::Op2(BinaryOp::Mul, b(konst(2.0)), b(konst(3.0)), Loc::default());
        let div = Expr::Op2(BinaryOp::Div, b(konst(3.0)), b(konst(2.0)), Loc::default());
        assert_eq!(value(&add, &[]), 5.0);
        assert_eq!(value(&sub, &[]), -1.0);
        assert_eq!(value(&mul, &[]), 6.0);
        assert_eq!(value(&div, &[]), 1.5);
    }

    #[test]
    fn lowers_nested_expr_with_var() {
        // births = population * birth_rate, with population in slot 4
        let expr = Expr::Op2(
            BinaryOp::Mul,
            b(Expr::Var(4, Loc::default())),
            b(konst(0.1)),
            Loc::default(),
        );
        assert_eq!(value(&expr, &[(32, 100.0)]), 10.0);
    }

    #[test]
    fn lowers_assign_curr_constant() {
        // store 42.0 into curr slot 5 (byte 40), read it back
        let expr = Expr::AssignCurr(5, b(konst(42.0)));
        assert_eq!(stored(&expr, &[], 40), 42.0);
    }

    #[test]
    fn lowers_assign_curr_from_expr() {
        // deaths = population / average_lifespan -> curr slot 6 (byte 48)
        let expr = Expr::AssignCurr(
            6,
            b(Expr::Op2(
                BinaryOp::Div,
                b(Expr::Var(4, Loc::default())),
                b(konst(80.0)),
                Loc::default(),
            )),
        );
        assert_eq!(stored(&expr, &[(32, 200.0)], 48), 2.5);
    }

    #[test]
    fn lowers_assign_next_euler_update() {
        // next[pop] = pop + (births - deaths) * dt, all read from curr.
        // pop=slot4, births=slot5, deaths=slot6; dt=0.5.
        // next slot 4 lives at next_base(4096) + 32 = 4128.
        let pop = || Expr::Var(4, Loc::default());
        let births = Expr::Var(5, Loc::default());
        let deaths = Expr::Var(6, Loc::default());
        let net = Expr::Op2(BinaryOp::Sub, b(births), b(deaths), Loc::default());
        let delta = Expr::Op2(
            BinaryOp::Mul,
            b(net),
            b(Expr::Dt(Loc::default())),
            Loc::default(),
        );
        let expr = Expr::AssignNext(
            4,
            b(Expr::Op2(BinaryOp::Add, b(pop()), b(delta), Loc::default())),
        );
        // pop=100, births=10, deaths=2.5 -> 100 + (7.5)*0.5 = 103.75
        let seed = &[(32, 100.0), (40, 10.0), (48, 2.5)];
        assert_eq!(stored(&expr, seed, 4128), 103.75);
    }

    #[test]
    fn lowers_unary_not_truthiness() {
        let not0 = Expr::Op1(UnaryOp::Not, b(konst(0.0)), Loc::default());
        let not5 = Expr::Op1(UnaryOp::Not, b(konst(5.0)), Loc::default());
        assert_eq!(value(&not0, &[]), 1.0);
        assert_eq!(value(&not5, &[]), 0.0);
    }

    #[test]
    fn lowers_comparisons_to_f64_bool() {
        let gt_true = Expr::Op2(BinaryOp::Gt, b(konst(2.0)), b(konst(1.0)), Loc::default());
        let gt_false = Expr::Op2(BinaryOp::Gt, b(konst(1.0)), b(konst(2.0)), Loc::default());
        let le_true = Expr::Op2(BinaryOp::Lte, b(konst(1.0)), b(konst(1.0)), Loc::default());
        assert_eq!(value(&gt_true, &[]), 1.0);
        assert_eq!(value(&gt_false, &[]), 0.0);
        assert_eq!(value(&le_true, &[]), 1.0);
    }

    #[test]
    fn lowers_if_then_else() {
        let if_true = Expr::If(
            b(konst(1.0)),
            b(konst(10.0)),
            b(konst(20.0)),
            Loc::default(),
        );
        let if_false = Expr::If(
            b(konst(0.0)),
            b(konst(10.0)),
            b(konst(20.0)),
            Loc::default(),
        );
        assert_eq!(value(&if_true, &[]), 10.0);
        assert_eq!(value(&if_false, &[]), 20.0);
    }

    #[test]
    fn lowers_if_with_comparison_condition() {
        // if population > 50 then 1 else 0, population in slot 4
        let cond = Expr::Op2(
            BinaryOp::Gt,
            b(Expr::Var(4, Loc::default())),
            b(konst(50.0)),
            Loc::default(),
        );
        let expr = Expr::If(b(cond), b(konst(1.0)), b(konst(0.0)), Loc::default());
        assert_eq!(value(&expr, &[(32, 100.0)]), 1.0);
        assert_eq!(value(&expr, &[(32, 10.0)]), 0.0);
    }

    #[test]
    fn lowers_time_builtin_reads_slot_zero() {
        let expr = Expr::App(BuiltinFn::Time, Loc::default());
        assert_eq!(value(&expr, &[(0, 13.0)]), 13.0);
    }

    #[test]
    fn lowers_time_constant_builtins() {
        assert_eq!(
            value(&Expr::App(BuiltinFn::TimeStep, Loc::default()), &[]),
            0.5
        );
        assert_eq!(
            value(&Expr::App(BuiltinFn::StartTime, Loc::default()), &[]),
            1.0
        );
        assert_eq!(
            value(&Expr::App(BuiltinFn::FinalTime, Loc::default()), &[]),
            25.0
        );
    }

    #[test]
    fn lowers_math_builtins() {
        let abs = Expr::App(BuiltinFn::Abs(b(konst(-4.0))), Loc::default());
        let sqrt = Expr::App(BuiltinFn::Sqrt(b(konst(9.0))), Loc::default());
        assert_eq!(value(&abs, &[]), 4.0);
        assert_eq!(value(&sqrt, &[]), 3.0);
    }

    #[test]
    fn unsupported_node_returns_error() {
        let expr = Expr::ModuleInput(0, Loc::default());
        let mut func = Function::new([]);
        let result = lower_expr(&expr, &ctx(), &mut func);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }
}
