// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the bytecode-to-WebAssembly lowering ([`super`]). Split out of
//! `lower.rs` to keep that file under the project line-count lint; this is the
//! `#[cfg(test)] mod tests` body, included via `#[path]` so `use super::*`
//! still resolves the lowering module's private items.

use super::*;
use checked::Store;
use wasm::validate;
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, FunctionSection, MemorySection, MemoryType, Module,
    TypeSection, ValType,
};

use crate::bytecode::ByteCodeContext;
use std::sync::OnceLock;

/// Local layout for the test harness function. The function takes
/// `module_off` as param 0; the scratch f64 and the condition i32(s) are
/// declared locals.
const L_MODULE_OFF: u32 = 0;
const L_SCRATCH: u32 = 1;
const L_COND_BASE: u32 = 2;

/// A shared empty `ByteCodeContext` for the scalar-opcode tests, which never
/// touch the array tables. Array-view tests build their own context (with
/// `static_views`/`temp_offsets`) and an `EmitCtx` borrowing it locally.
fn empty_ctx() -> &'static ByteCodeContext {
    static EMPTY: OnceLock<ByteCodeContext> = OnceLock::new();
    EMPTY.get_or_init(ByteCodeContext::default)
}

fn ctx_with_cond_depth(depth: usize) -> EmitCtx<'static> {
    EmitCtx {
        curr_base: 0,
        next_base: 4096,
        // The non-Lookup opcode tests place no GF regions; these bases are
        // unused by the opcodes they exercise. The Lookup-opcode tests
        // (which do read these) build their own ctx with real GF bases.
        gf_directory_base: 0,
        gf_data_base: 0,
        // The PREVIOUS/INIT opcode tests build their own ctx with real
        // snapshot bases + flag; the rest never touch these fields.
        initial_values_base: 0,
        prev_values_base: 0,
        use_prev_fallback_global: 0,
        step_part: StepPart::Flows,
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
        // The scalar-opcode tests place no temp region; the array-view tests
        // build their own ctx with a real temp base + context.
        temp_storage_base: 0,
        // Dynamic-subscript scratch i32 locals (Task 4) follow the scratch
        // f64 / condition i32s / Apply f64s / the vector-op scratch blocks;
        // `build_module` declares exactly `count_extra_i32_locals(bc)` of them
        // at this base.
        extra_i32_local_base: extra_i32_local_base(depth),
        // The fixed Phase-6 vector-op scratch local blocks.
        vector_f64_locals: vector_f64_locals_for(depth),
        vector_i32_locals: vector_i32_locals_for(depth),
        // The vector-op scratch region: well past TEMP_BASE (8192) but within
        // the harness's single 64 KiB memory page, so the small test views'
        // sort-pair / collected-value staging never collides with temp_storage.
        vector_scratch_base: VECTOR_SCRATCH_BASE,
        ctx: empty_ctx(),
    }
}

/// Byte offset of the vector-op scratch region for the test harness. Past
/// `TEMP_BASE` (8192) and any small test temp region, with ~6000 f64 slots of
/// headroom before the 64 KiB page end -- ample for the tiny test views.
const VECTOR_SCRATCH_BASE: u32 = 16384;

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
    // 1 scratch f64 local, `cond_depth` i32 condition locals, the 3 `Apply`
    // scratch f64 locals, and the program's dynamic-subscript i32 scratch
    // locals -- the same layout production uses.
    let mut func = Function::new(opcode_fn_locals(cond_depth, count_extra_i32_locals(bc)));
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
fn lookup_lowers_without_error() {
    // Lookup is supported as of Phase 3; lowering must succeed where Phase 2
    // returned Unsupported. (Numeric parity is covered by the seeded-table
    // tests below and the end-to-end GF model tests in module.rs.)
    let mut func = Function::new(opcode_fn_locals(0, 0));
    let program = bc(
        vec![0.0, 1.0],
        vec![
            Opcode::LoadConstant { id: 0 }, // element_offset
            Opcode::LoadConstant { id: 1 }, // index
            Opcode::Lookup {
                base_gf: 0,
                table_count: 1,
                mode: LookupMode::Interpolate,
            },
        ],
    );
    let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
    assert!(result.is_ok(), "Lookup should lower without error");
}

#[test]
fn unsupported_array_opcode_returns_error() {
    // The reducers, static view ops, and iteration loops are supported as of
    // Phase 5 Tasks 1-3, so this drives a still-unsupported module opcode
    // (`EvalModule`, Phase 7) to confirm an unhandled opcode still returns a
    // clean error rather than a wrong module.
    let mut func = Function::new([]);
    let program = bc(vec![], vec![Opcode::EvalModule { id: 0, n_inputs: 0 }]);
    let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
    assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
}

#[test]
fn begin_iter_on_empty_view_stack_errors() {
    // A `BeginIter` with no view pushed first is malformed bytecode: it must
    // error cleanly (empty-view-stack), not panic.
    let mut func = Function::new([]);
    let program = bc(
        vec![],
        vec![Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: false,
        }],
    );
    let result = emit_bytecode(&program, &ctx_with_cond_depth(0), &mut func);
    assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
}

// ── Lookup opcode: seeded-table parity with the VM lookup functions ───

// GF region bases for the Lookup opcode tests, placed well past
// `next_base` (4096) so they cannot overlap the curr/next chunks. The
// single test table's directory entry sits at `GF_DIR_BASE`; its data
// follows at `GF_DATA_BASE`.
const GF_DIR_BASE: u32 = 8192;
const GF_DATA_BASE: u32 = 8192 + 8; // one 8-byte directory entry

/// A ctx whose GF region bases point at the hand-seeded test regions, so a
/// `Lookup` opcode reads the directory at `GF_DIR_BASE`.
fn ctx_with_gf() -> EmitCtx<'static> {
    EmitCtx {
        gf_directory_base: GF_DIR_BASE,
        gf_data_base: GF_DATA_BASE,
        ..ctx_with_cond_depth(0)
    }
}

/// Pack a GF directory entry `(data_off, count)` into the f64 whose 8 LE
/// bytes are `data_off` (low i32) then `count` (high i32) -- so seeding it as
/// one f64 writes exactly the two i32 the `Lookup` opcode reads.
///
/// Assumes a little-endian test host: the low 32 bits land at the lower
/// address, matching production's `to_le_bytes` directory encoding (the
/// opcode reads `data_off` at offset 0 and `count` at offset 4).
fn dir_entry_f64(data_off: u32, count: u32) -> f64 {
    f64::from_bits(((count as u64) << 32) | data_off as u64)
}

/// Seed a single GF table (`base_gf == 0`, `table_count == 1`) into memory:
/// the directory entry at `GF_DIR_BASE` and the knots at `GF_DATA_BASE`.
fn seed_single_table(knots: &[(f64, f64)]) -> Vec<(u64, f64)> {
    let mut seed = vec![(
        u64::from(GF_DIR_BASE),
        dir_entry_f64(GF_DATA_BASE, knots.len() as u32),
    )];
    for (k, &(x, y)) in knots.iter().enumerate() {
        let knot_addr = u64::from(GF_DATA_BASE) + (k as u64) * 16;
        seed.push((knot_addr, x));
        seed.push((knot_addr + 8, y));
    }
    seed
}

/// Run a `Lookup` over a single seeded table at `(element_offset, index)`.
/// `table_count` lets a test push an out-of-range element_offset.
fn run_lookup_opcode(
    mode: LookupMode,
    knots: &[(f64, f64)],
    table_count: u16,
    element_offset: f64,
    index: f64,
) -> f64 {
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // element_offset (pushed first)
        Opcode::LoadConstant { id: 1 }, // index (pushed second, on top)
        Opcode::Lookup {
            base_gf: 0,
            table_count,
            mode,
        },
    ];
    run(
        &bc(vec![element_offset, index], code),
        &ctx_with_gf(),
        true,
        0,
        &seed_single_table(knots),
        None,
    )
}

/// The VM oracle for `mode` -- the exact function the opcode dispatches to.
fn vm_lookup_oracle(mode: LookupMode, knots: &[(f64, f64)], index: f64) -> f64 {
    match mode {
        LookupMode::Interpolate => crate::vm::lookup(knots, index),
        LookupMode::Forward => crate::vm::lookup_forward(knots, index),
        LookupMode::Backward => crate::vm::lookup_backward(knots, index),
    }
}

fn assert_lookup_opcode_matches_vm(mode: LookupMode, knots: &[(f64, f64)], index: f64) {
    let got = run_lookup_opcode(mode, knots, 1, 0.0, index);
    let want = vm_lookup_oracle(mode, knots, index);
    if want.is_nan() {
        assert!(got.is_nan(), "{mode:?} at {index}: expected NaN, got {got}");
    } else {
        assert_eq!(got, want, "{mode:?} at {index}: got {got}, want {want}");
    }
}

const LOOKUP_OPCODE_TABLE: &[(f64, f64)] = &[(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];

#[test]
fn lookup_opcode_dispatches_to_each_mode_and_reads_directory() {
    // The opcode reads (data_off, count) from the directory, then dispatches
    // to the mode's helper. Probe below/above range, on a knot, and between
    // knots for all three modes against the VM oracle.
    let probes = [-1.0, 0.0, 0.5, 1.0, 1.75, 2.5, 3.0, 4.0, 9.0];
    for mode in [
        LookupMode::Interpolate,
        LookupMode::Forward,
        LookupMode::Backward,
    ] {
        for &index in &probes {
            assert_lookup_opcode_matches_vm(mode, LOOKUP_OPCODE_TABLE, index);
        }
    }
}

#[test]
fn lookup_opcode_out_of_range_element_offset_is_nan() {
    // The VM pushes NaN when element_offset < 0 or >= table_count, BEFORE
    // touching the table; the opcode must match (the directory is seeded for
    // table 0 only, so an OOB offset must short-circuit, never read garbage).
    for mode in [
        LookupMode::Interpolate,
        LookupMode::Forward,
        LookupMode::Backward,
    ] {
        // table_count = 1, so offset 1 and -1 are both out of range.
        assert!(
            run_lookup_opcode(mode, LOOKUP_OPCODE_TABLE, 1, 1.0, 2.0).is_nan(),
            "{mode:?}: element_offset == table_count must be NaN"
        );
        assert!(
            run_lookup_opcode(mode, LOOKUP_OPCODE_TABLE, 1, -1.0, 2.0).is_nan(),
            "{mode:?}: negative element_offset must be NaN"
        );
        // In range (offset 0) is NOT NaN for an in-range index.
        assert!(
            !run_lookup_opcode(mode, LOOKUP_OPCODE_TABLE, 1, 0.0, 2.0).is_nan(),
            "{mode:?}: in-range element_offset must not be NaN"
        );
    }
}

#[test]
fn lookup_opcode_nan_index_is_nan() {
    for mode in [
        LookupMode::Interpolate,
        LookupMode::Forward,
        LookupMode::Backward,
    ] {
        assert!(
            run_lookup_opcode(mode, LOOKUP_OPCODE_TABLE, 1, 0.0, f64::NAN).is_nan(),
            "{mode:?}: a NaN index must be NaN"
        );
    }
}

// ── Lookup opcode: runtime table selection across TWO tables ──────────
//
// The single-table parity tests above always pass `element_offset == 0`, so
// the directory-indexing arithmetic in `push_gf_directory_addr`
// (`gf_directory_base + (base_gf + element_offset) * 8`) is only exercised
// for offset 0 -- the `* 8` stride and the offset add are never tested with
// a nonzero offset (the out-of-range tests short-circuit to NaN before the
// directory read). Phase 5/7 lower an arrayed scalar `Lookup` to a runtime
// per-element `element_offset` that selects a per-element table, so the
// table-selection path must be pinned here.

// Two-table layout: a 2-entry directory at `GF2_DIR_BASE`, then each
// table's knots laid out back-to-back past the directory.
const GF2_DIR_BASE: u32 = 8192;
const GF2_TABLE0_DATA: u32 = GF2_DIR_BASE + 2 * 8; // past two 8-byte entries
// Table 0 has two knots (4 f64 = 32 bytes); table 1's data follows.
const GF2_TABLE1_DATA: u32 = GF2_TABLE0_DATA + 2 * 16;

/// Seed two GF tables so that directory entry `t` (`t ∈ {0,1}`) points at
/// `table_t`'s knots. Mirrors the production directory layout the opcode
/// reads via `push_gf_directory_addr`.
fn seed_two_tables(table0: &[(f64, f64)], table1: &[(f64, f64)]) -> Vec<(u64, f64)> {
    let mut seed = vec![
        (
            u64::from(GF2_DIR_BASE),
            dir_entry_f64(GF2_TABLE0_DATA, table0.len() as u32),
        ),
        (
            u64::from(GF2_DIR_BASE) + 8,
            dir_entry_f64(GF2_TABLE1_DATA, table1.len() as u32),
        ),
    ];
    for (base, knots) in [(GF2_TABLE0_DATA, table0), (GF2_TABLE1_DATA, table1)] {
        for (k, &(x, y)) in knots.iter().enumerate() {
            let knot_addr = u64::from(base) + (k as u64) * 16;
            seed.push((knot_addr, x));
            seed.push((knot_addr + 8, y));
        }
    }
    seed
}

/// Run a `Lookup` with a compile-time-constant `element_offset` against a
/// two-table directory (`base_gf == 0`, `table_count == 2`).
fn run_lookup_two_tables(
    mode: LookupMode,
    table0: &[(f64, f64)],
    table1: &[(f64, f64)],
    element_offset: f64,
    index: f64,
) -> f64 {
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // element_offset (pushed first)
        Opcode::LoadConstant { id: 1 }, // index (pushed second, on top)
        Opcode::Lookup {
            base_gf: 0,
            table_count: 2,
            mode,
        },
    ];
    let ctx = EmitCtx {
        gf_directory_base: GF2_DIR_BASE,
        // `gf_data_base` is unused at runtime by the opcode (each table's
        // data offset comes from its directory entry), but set it to the
        // first table's data so the ctx is internally consistent.
        gf_data_base: GF2_TABLE0_DATA,
        ..ctx_with_cond_depth(0)
    };
    run(
        &bc(vec![element_offset, index], code),
        &ctx,
        true,
        0,
        &seed_two_tables(table0, table1),
        None,
    )
}

#[test]
fn lookup_opcode_selects_table_by_element_offset() {
    // Two tables whose values differ at the probe index in ALL three modes,
    // so selecting the wrong table is observable regardless of mode:
    //   table 0: y = 10x        index 5 -> interp 50,  fwd 100, bwd 0
    //   table 1: y = x/10 + 1   index 5 -> interp 1.5, fwd 2,   bwd 1
    let table0: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 100.0)];
    let table1: &[(f64, f64)] = &[(0.0, 1.0), (10.0, 2.0)];
    let index = 5.0;

    for mode in [
        LookupMode::Interpolate,
        LookupMode::Forward,
        LookupMode::Backward,
    ] {
        // The two tables must genuinely disagree here, otherwise selecting
        // the wrong table would silently pass.
        let want0 = vm_lookup_oracle(mode, table0, index);
        let want1 = vm_lookup_oracle(mode, table1, index);
        assert_ne!(
            want0, want1,
            "{mode:?}: tables must differ at the probe index to detect mis-selection"
        );

        // element_offset == 1 selects table 1; the result must match the VM
        // oracle over table 1 (and therefore differ from table 0).
        let got = run_lookup_two_tables(mode, table0, table1, 1.0, index);
        assert_eq!(
            got, want1,
            "{mode:?}: element_offset==1 must read table 1: got {got}, want {want1}"
        );

        // Sanity: element_offset == 0 still selects table 0 (the offset is a
        // real selector, not a constant remap to table 1).
        let got0 = run_lookup_two_tables(mode, table0, table1, 0.0, index);
        assert_eq!(
            got0, want0,
            "{mode:?}: element_offset==0 must read table 0: got {got0}, want {want0}"
        );
    }
}

// ── LoadInitial / LoadPrev opcodes (Task 1: snapshot regions) ─────────

// Snapshot region bases for these tests, placed past `next_base` (4096) so
// they cannot overlap the curr/next chunks.
const INITIAL_BASE: u32 = 8192;
const PREV_BASE: u32 = 8192 + 4096;

/// `LoadInitial` in the flows/stocks programs reads `initial_values[off]`
/// (the post-initials snapshot), NOT `curr`. Seed both regions to distinct
/// values at the same slot so a wrong-region read is observable.
#[test]
fn load_initial_in_flows_reads_initial_values_region() {
    let ctx = EmitCtx {
        initial_values_base: INITIAL_BASE,
        step_part: StepPart::Flows,
        ..ctx_with_cond_depth(0)
    };
    // curr[2] = 111 (byte 16), initial_values[2] = 222 (INITIAL_BASE + 16).
    let seed = [(16u64, 111.0), (u64::from(INITIAL_BASE) + 16, 222.0)];
    let got = run(
        &bc(vec![], vec![Opcode::LoadInitial { off: 2 }]),
        &ctx,
        true,
        0,
        &seed,
        None,
    );
    assert_eq!(got, 222.0, "LoadInitial in Flows must read initial_values");
}

/// `LoadInitial` in the initials program reads `curr[off]` (the value being
/// computed), because the snapshot is not yet taken (`vm.rs:1334`).
#[test]
fn load_initial_in_initials_reads_curr() {
    let ctx = EmitCtx {
        initial_values_base: INITIAL_BASE,
        step_part: StepPart::Initials,
        ..ctx_with_cond_depth(0)
    };
    let seed = [(16u64, 111.0), (u64::from(INITIAL_BASE) + 16, 222.0)];
    let got = run(
        &bc(vec![], vec![Opcode::LoadInitial { off: 2 }]),
        &ctx,
        true,
        0,
        &seed,
        None,
    );
    assert_eq!(got, 111.0, "LoadInitial in Initials must read curr");
}

/// `LoadInitial` honors `module_off`: with a non-zero module base it reads
/// `initial_values[module_off + off]`.
#[test]
fn load_initial_honors_module_off() {
    let ctx = EmitCtx {
        initial_values_base: INITIAL_BASE,
        step_part: StepPart::Stocks,
        ..ctx_with_cond_depth(0)
    };
    // module_off=2, off=1 -> initial_values[3] at INITIAL_BASE + 24.
    let program = bc(vec![], vec![Opcode::LoadInitial { off: 1 }]);
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
    store.mem_access_mut_slice(mem, |bytes| {
        let a = (INITIAL_BASE + 24) as usize;
        bytes[a..a + 8].copy_from_slice(&77.0_f64.to_le_bytes());
    });
    let eval = store
        .instance_export(module, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    let result: f64 = store.invoke_simple_typed(eval, (2_i32,)).expect("invoke");
    assert_eq!(
        result, 77.0,
        "LoadInitial must read initial_values[module_off+off]"
    );
}

/// Build a module exporting `mem`, a mutable i32 global `use_prev_fallback`
/// (at index 0, the index the test ctx names), and an `eval(module_off: i32)
/// -> f64` whose body lowers `LoadConstant(fallback); LoadPrev{off}`. The
/// helper functions lead the function/code sections so any `call` resolves;
/// `eval` follows. `fallback_flag` is the global's init value (1 = use the
/// fallback, 0 = read prev_values).
fn build_load_prev_module(off: u16, fallback: f64, fallback_flag: i32) -> Vec<u8> {
    let mut module = Module::new();
    let helpers = build_helpers();
    let n_helpers = helpers.functions.len() as u32;

    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::F64]); // eval
    for hf in &helpers.functions {
        types.ty().function(hf.params.clone(), hf.results.clone());
    }
    module.section(&types);

    let mut functions = FunctionSection::new();
    for (i, _) in helpers.functions.iter().enumerate() {
        functions.function(1 + i as u32);
    }
    functions.function(0); // eval -> type 0
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

    // The single mutable i32 global the LoadPrev ctx gates on (index 0).
    let mut globals = wasm_encoder::GlobalSection::new();
    globals.global(
        wasm_encoder::GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &wasm_encoder::ConstExpr::i32_const(fallback_flag),
    );
    module.section(&globals);

    let mut exports = ExportSection::new();
    exports.export("eval", ExportKind::Func, n_helpers);
    exports.export("mem", ExportKind::Memory, 0);
    module.section(&exports);

    let ctx = EmitCtx {
        prev_values_base: PREV_BASE,
        use_prev_fallback_global: 0,
        ..ctx_with_cond_depth(0)
    };
    let program = bc(
        vec![fallback],
        vec![Opcode::LoadConstant { id: 0 }, Opcode::LoadPrev { off }],
    );

    let mut code = CodeSection::new();
    for hf in helpers.functions {
        code.function(&hf.body);
    }
    let mut func = Function::new(opcode_fn_locals(0, 0));
    emit_bytecode(&program, &ctx, &mut func).expect("LoadPrev should lower");
    func.instruction(&Instruction::End);
    code.function(&func);
    module.section(&code);

    module.finish()
}

/// Run `LoadConstant(fallback); LoadPrev{off}` with `prev_values[off]` seeded
/// to `prev_value` and the gate set to `fallback_flag`.
fn run_load_prev(off: u16, fallback: f64, prev_value: f64, fallback_flag: i32) -> f64 {
    let bytes = build_load_prev_module(off, fallback, fallback_flag);
    let info = validate(&bytes).expect("LoadPrev module must validate");
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
    store.mem_access_mut_slice(mem, |bytes| {
        let a = (PREV_BASE + u32::from(off) * SLOT_SIZE) as usize;
        bytes[a..a + 8].copy_from_slice(&prev_value.to_le_bytes());
    });
    let eval = store
        .instance_export(module, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    store.invoke_simple_typed(eval, (0_i32,)).expect("invoke")
}

/// `LoadPrev` returns the caller-supplied fallback while `use_prev_fallback`
/// is set (1), exactly as the VM does before the first snapshot
/// (`vm.rs:1322`). The seeded `prev_values` value must NOT be read.
#[test]
fn load_prev_returns_fallback_when_flag_set() {
    let got = run_load_prev(2, 3.5, 999.0, 1);
    assert_eq!(got, 3.5, "with the flag set, LoadPrev yields its fallback");
}

/// `LoadPrev` reads `prev_values[off]` once `use_prev_fallback` is cleared
/// (0), exactly as the VM does after the first snapshot (`vm.rs:1325`).
#[test]
fn load_prev_reads_prev_values_when_flag_clear() {
    let got = run_load_prev(2, 3.5, 999.0, 0);
    assert_eq!(
        got, 999.0,
        "with the flag clear, LoadPrev reads prev_values"
    );
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

    // The GF lookup helpers (`super::lookup`) `f64.load` from memory 0, so
    // a module that includes every helper body must declare a memory even
    // though `eq` itself never touches it.
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

// ════════════════════════════════════════════════════════════════════════
// Phase 5 Task 1: temp-element reads (LoadTempConst / LoadTempDynamic)
//
// The compile-time view-descriptor stack + the static view ops' addressing
// are pinned directly against the VM's `RuntimeView` in `views.rs`'s unit
// tests (no wasm or reducer needed); here the LoadTemp opcodes -- which read
// `temp_storage` and produce a value on the arithmetic stack -- are run under
// DLR-FT to confirm the emitted reads hit the temp region the VM addresses.
// ════════════════════════════════════════════════════════════════════════

// Region base for the temp-storage reads: well past `next_base` (4096) so it
// cannot overlap the curr/next chunks.
const TEMP_BASE: u32 = 8192;

/// Build an `EmitCtx` over a real `ByteCodeContext` (so the temp opcodes can
/// resolve `temp_offsets`), with `temp_storage_base` set.
fn ctx_with_arrays(context: &ByteCodeContext) -> EmitCtx<'_> {
    EmitCtx {
        temp_storage_base: TEMP_BASE,
        ctx: context,
        ..ctx_with_cond_depth(0)
    }
}

#[test]
fn load_temp_const_reads_temp_storage() {
    // temp_offsets = [0, 4]; LoadTempConst{temp_id:1, index:2} reads
    // temp_storage[4 + 2] = temp slot 6 (byte TEMP_BASE + 6*8).
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0, 4], 8);
    let ctx = ctx_with_arrays(&context);
    let code = vec![Opcode::LoadTempConst {
        temp_id: 1,
        index: 2,
    }];
    let seed = vec![(u64::from(TEMP_BASE) + 6 * 8, 42.0)];
    let got = run(&bc(vec![], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 42.0);
}

#[test]
fn load_temp_dynamic_reads_temp_storage() {
    // LoadTempDynamic{temp_id:0} pops a runtime index (floor) and reads
    // temp_storage[temp_offsets[0] + index]. Push index 3 via a constant.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 5);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // index = 3.0
        Opcode::LoadTempDynamic { temp_id: 0 },
    ];
    let seed = vec![(u64::from(TEMP_BASE) + 3 * 8, 77.0)];
    let got = run(&bc(vec![3.0], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 77.0);
}

#[test]
fn load_temp_dynamic_floors_fractional_index() {
    // The VM does `stack.pop().floor() as usize`; index 2.9 -> slot 2.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 4);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::LoadConstant { id: 0 },
        Opcode::LoadTempDynamic { temp_id: 0 },
    ];
    let seed = vec![(u64::from(TEMP_BASE) + 2 * 8, 13.0)];
    let got = run(&bc(vec![2.9], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 13.0);
}

// ════════════════════════════════════════════════════════════════════════
// Phase 5 Task 2: array reducers (Sum/Max/Min/Mean/Stddev/Size)
//
// These run the emitted reducers under DLR-FT and assert the result matches
// the VM's own addressing oracle (`RuntimeView::flat_offset`, via
// `StaticArrayView::to_runtime_view`) folded per the matching VM reducer arm
// (`vm.rs:2216-2309`). The view transform opcodes the production codegen does
// not emit directly (it bakes constant subscripts into one `PushStaticView`)
// are exercised here on a `PushVarView` base so each `apply_*` is reduced
// over and checked against the VM. Reuses `TEMP_BASE` / `ctx_with_arrays`
// from the Task 1 section above.
// ════════════════════════════════════════════════════════════════════════

use crate::bytecode::{
    DimensionInfo, RuntimeSparseMapping, RuntimeView, StaticArrayView, SubdimensionRelation,
};
use smallvec::SmallVec;

fn seed_run(base_byte: u64, values: &[f64]) -> Vec<(u64, f64)> {
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| (base_byte + (i as u64) * 8, v))
        .collect()
}

/// Read element `iter_idx` of `view` from a flat slab `data` indexed by slot,
/// using the VM's own addressing (`to_runtime_view().flat_offset`). The
/// addressing oracle for every reducer parity check.
fn vm_view_element(view: &StaticArrayView, data: &[f64], iter_idx: usize) -> f64 {
    let rv = view.to_runtime_view();
    let n = rv.dims.len();
    let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n];
    let mut remaining = iter_idx;
    for d in (0..n).rev() {
        let dim = rv.dims[d] as usize;
        indices[d] = (remaining % dim) as u16;
        remaining /= dim;
    }
    let flat = rv.flat_offset(&indices);
    data[rv.base_off as usize + flat]
}

/// The VM's expected `ArraySum` over `view`'s elements drawn from `data`.
fn vm_sum(view: &StaticArrayView, data: &[f64]) -> f64 {
    (0..view.to_runtime_view().size())
        .map(|i| vm_view_element(view, data, i))
        .sum()
}

fn dense_view(base_off: u32, dims: &[u16]) -> StaticArrayView {
    // Row-major strides for a dense contiguous array.
    let mut strides: SmallVec<[i32; 4]> = SmallVec::new();
    let mut s = 1i32;
    for &d in dims.iter().rev() {
        strides.push(s);
        s *= d as i32;
    }
    strides.reverse();
    StaticArrayView {
        base_off,
        is_temp: false,
        dims: dims.iter().copied().collect(),
        strides,
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: dims.iter().map(|_| 0u16).collect(),
    }
}

/// Compile+run `PushStaticView(view); <reduce>; PopView` over a `curr` array
/// seeded from `data` (slot 0 of curr is byte 0).
fn run_static_reduce(view: StaticArrayView, reduce: Opcode, data: &[f64]) -> f64 {
    let mut context = ByteCodeContext::default();
    let view_id = context.add_static_view(view);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id },
        reduce,
        Opcode::PopView {},
    ];
    run(&bc(vec![], code), &ctx, true, 0, &seed_run(0, data), None)
}
// ── Task 1: PushStaticView addressing across geometries ───────────────

#[test]
fn static_view_sum_contiguous_matches_vm() {
    // A bare 1-D contiguous view over curr slots 0..4.
    let data = [10.0, 20.0, 30.0, 40.0];
    let view = dense_view(0, &[4]);
    let got = run_static_reduce(view.clone(), Opcode::ArraySum {}, &data);
    assert_eq!(got, vm_sum(&view, &data));
    assert_eq!(got, 100.0);
}

#[test]
fn static_view_sum_with_offset_matches_vm() {
    // A range slice source[3:5] over a 5-element array bakes into `offset=2`
    // (0-based start), dims=[3]. Elements are data[2], data[3], data[4].
    let data = [1.0, 2.0, 3.0, 4.0, 5.0];
    let mut view = dense_view(0, &[3]);
    view.offset = 2;
    let got = run_static_reduce(view.clone(), Opcode::ArraySum {}, &data);
    assert_eq!(got, vm_sum(&view, &data));
    assert_eq!(got, 3.0 + 4.0 + 5.0);
}

#[test]
fn static_view_sum_transposed_strides_matches_vm() {
    // A 2x3 matrix stored row-major (strides [3,1]) transposed to dims [3,2]
    // with strides [1,3] -- non-contiguous, so the strided flat_offset path
    // is exercised. Data laid out row-major: m[r,c] = data[r*3 + c].
    let data = [11.0, 12.0, 13.0, 21.0, 22.0, 23.0];
    let view = StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[3, 2]),
        strides: SmallVec::from_slice(&[1, 3]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0, 0]),
    };
    assert!(!view.to_runtime_view().is_contiguous());
    let got = run_static_reduce(view.clone(), Opcode::ArraySum {}, &data);
    // Sum is order-independent and covers all six cells regardless.
    assert_eq!(got, vm_sum(&view, &data));
    assert_eq!(got, 11.0 + 12.0 + 13.0 + 21.0 + 22.0 + 23.0);
}

#[test]
fn static_view_max_transposed_picks_right_cells() {
    // Max over the transposed view must read the same cells the VM reads.
    // Make one cell dominate so a mis-addressed read would change the max.
    let data = [11.0, 12.0, 99.0, 21.0, 22.0, 23.0];
    let view = StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[3, 2]),
        strides: SmallVec::from_slice(&[1, 3]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0, 0]),
    };
    let got = run_static_reduce(view, Opcode::ArrayMax {}, &data);
    assert_eq!(got, 99.0);
}

#[test]
fn static_view_sum_sparse_matches_vm() {
    // A sparse (star-range) view selecting elements at parent offsets [0, 2]
    // of a 4-element array: dims=[2], a RuntimeSparseMapping mapping view
    // index 0->parent 0, 1->parent 2. Elements are data[0], data[2].
    let data = [5.0, 6.0, 7.0, 8.0];
    let view = StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[2]),
        strides: SmallVec::from_slice(&[1]),
        offset: 0,
        sparse: smallvec::smallvec![RuntimeSparseMapping {
            dim_index: 0,
            parent_offsets: SmallVec::from_slice(&[0, 2]),
        }],
        dim_ids: SmallVec::from_slice(&[0]),
    };
    let got = run_static_reduce(view.clone(), Opcode::ArraySum {}, &data);
    assert_eq!(got, vm_sum(&view, &data));
    assert_eq!(got, 5.0 + 7.0);
}

#[test]
fn static_temp_view_sum_reads_temp_storage() {
    // A contiguous temp view (is_temp) reads temp_storage, not curr. temp_id
    // 0 lives at temp_offsets[0]=0, so its slot 0 is byte TEMP_BASE.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 3);
    let view = StaticArrayView {
        base_off: 0, // temp_id 0
        is_temp: true,
        dims: SmallVec::from_slice(&[3]),
        strides: SmallVec::from_slice(&[1]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0]),
    };
    let view_id = context.add_static_view(view);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id },
        Opcode::ArraySum {},
        Opcode::PopView {},
    ];
    // Seed curr slots 0..3 with decoys and temp_storage with the real data;
    // a read from the wrong region would pick up the decoys.
    let mut seed = seed_run(0, &[100.0, 200.0, 300.0]);
    seed.extend(seed_run(u64::from(TEMP_BASE), &[2.0, 3.0, 4.0]));
    let got = run(&bc(vec![], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 9.0, "temp view must read temp_storage, not curr");
}

#[test]
fn static_temp_view_honors_temp_offset() {
    // temp_id 1 lives at temp_offsets[1]=4, so its slot 0 is byte
    // TEMP_BASE + 4*8. A reducer over it must skip temp 0's slots.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0, 4], 6);
    let view = StaticArrayView {
        base_off: 1, // temp_id 1
        is_temp: true,
        dims: SmallVec::from_slice(&[2]),
        strides: SmallVec::from_slice(&[1]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0]),
    };
    let view_id = context.add_static_view(view);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id },
        Opcode::ArraySum {},
        Opcode::PopView {},
    ];
    // temp_storage: [t0_0, t0_1, t0_2, t0_3, t1_0, t1_1] = [9,9,9,9, 2, 5].
    let seed = seed_run(u64::from(TEMP_BASE), &[9.0, 9.0, 9.0, 9.0, 2.0, 5.0]);
    let got = run(&bc(vec![], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 7.0, "temp view must start at temp_offsets[temp_id]");
}

// ── Task 1: view transform opcodes (mirror RuntimeView::apply_*) ──────
//
// Build a full var view with PushVarView, apply one transform, reduce, and
// compare to the VM's RuntimeView with the same transform applied. These are
// the opcodes production codegen bakes into a single PushStaticView, so they
// are exercised here directly to pin each `apply_*` against the VM.

/// A `ByteCodeContext` with a single dimension of `size` (DimId 0) and a
/// dim-list `[DimId 0]` (DimListId 0) for a 1-D `PushVarView`.
fn ctx_one_dim(size: u16) -> ByteCodeContext {
    let mut context = ByteCodeContext::default();
    let name_id = context.intern_name("D");
    context.add_dimension(DimensionInfo::indexed(name_id, size));
    context.add_dim_list(1, [0, 0, 0, 0]);
    context
}

/// Run `PushVarView(base 0, dims) ; <transforms> ; <reduce> ; PopView` and
/// also build the VM `RuntimeView` the same way for the addressing oracle.
fn run_var_view_reduce(
    context: &ByteCodeContext,
    transforms: &[Opcode],
    reduce: Opcode,
    data: &[f64],
) -> f64 {
    let ctx = ctx_with_arrays(context);
    let mut code = vec![Opcode::PushVarView {
        base_off: 0,
        dim_list_id: 0,
    }];
    code.extend_from_slice(transforms);
    code.push(reduce);
    code.push(Opcode::PopView {});
    run(&bc(vec![], code), &ctx, true, 0, &seed_run(0, data), None)
}

#[test]
fn view_subscript_const_drops_dim_matches_vm() {
    // A 2x3 matrix; subscript dim 0 to index 1 (0-based) -> row 1: cells
    // data[3], data[4], data[5]. Mirror with RuntimeView.
    let mut context = ByteCodeContext::default();
    let name_d = context.intern_name("D");
    context.add_dimension(DimensionInfo::indexed(name_d, 2));
    let name_e = context.intern_name("E");
    context.add_dimension(DimensionInfo::indexed(name_e, 3));
    context.add_dim_list(2, [0, 1, 0, 0]); // [DimId 0 (size2), DimId 1 (size3)]
    let data = [11.0, 12.0, 13.0, 21.0, 22.0, 23.0];

    let got = run_var_view_reduce(
        &context,
        &[Opcode::ViewSubscriptConst {
            dim_idx: 0,
            index: 1,
        }],
        Opcode::ArraySum {},
        &data,
    );
    // VM oracle: build the same RuntimeView and apply the same subscript.
    let mut rv = RuntimeView::for_var(
        0,
        SmallVec::from_slice(&[2, 3]),
        SmallVec::from_slice(&[0, 1]),
    );
    rv.apply_single_subscript(0, 1);
    let want: f64 = (0..rv.size())
        .map(|i| {
            let n = rv.dims.len();
            let mut idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; n];
            let mut rem = i;
            for d in (0..n).rev() {
                idx[d] = (rem % rv.dims[d] as usize) as u16;
                rem /= rv.dims[d] as usize;
            }
            data[rv.base_off as usize + rv.flat_offset(&idx)]
        })
        .sum();
    assert_eq!(got, want);
    assert_eq!(got, 21.0 + 22.0 + 23.0);
}

#[test]
fn view_range_matches_vm() {
    // 1-D dim of 5; ViewRange [1:4) keeps indices 1,2,3 -> data[1..4].
    let context = ctx_one_dim(5);
    let data = [1.0, 2.0, 3.0, 4.0, 5.0];
    let got = run_var_view_reduce(
        &context,
        &[Opcode::ViewRange {
            dim_idx: 0,
            start: 1,
            end: 4,
        }],
        Opcode::ArraySum {},
        &data,
    );
    assert_eq!(got, 2.0 + 3.0 + 4.0);
}

#[test]
fn view_wildcard_is_noop() {
    // ViewWildcard leaves the dimension as-is: the sum is the full array.
    let context = ctx_one_dim(4);
    let data = [1.0, 2.0, 3.0, 4.0];
    let got = run_var_view_reduce(
        &context,
        &[Opcode::ViewWildcard { dim_idx: 0 }],
        Opcode::ArraySum {},
        &data,
    );
    assert_eq!(got, 10.0);
}

#[test]
fn view_transpose_then_reduce_matches_vm() {
    // 2x3 matrix; transpose to 3x2 then sum (order-independent but exercises
    // the stride/dim reversal addressing).
    let mut context = ByteCodeContext::default();
    let name_d = context.intern_name("D");
    context.add_dimension(DimensionInfo::indexed(name_d, 2));
    let name_e = context.intern_name("E");
    context.add_dimension(DimensionInfo::indexed(name_e, 3));
    context.add_dim_list(2, [0, 1, 0, 0]);
    let data = [11.0, 12.0, 13.0, 21.0, 22.0, 23.0];
    let got = run_var_view_reduce(
        &context,
        &[Opcode::ViewTranspose {}],
        Opcode::ArraySum {},
        &data,
    );
    assert_eq!(got, 11.0 + 12.0 + 13.0 + 21.0 + 22.0 + 23.0);
}

#[test]
fn view_star_range_sparse_matches_vm() {
    // A 1-D parent dim of 4; a star-range via a subdim relation selecting
    // parent offsets [1, 3] -> sum of data[1] + data[3].
    let mut context = ByteCodeContext::default();
    let name_p = context.intern_name("P");
    context.add_dimension(DimensionInfo::indexed(name_p, 4));
    let name_s = context.intern_name("S");
    context.add_dimension(DimensionInfo::indexed(name_s, 2)); // child dim
    context.add_dim_list(1, [0, 0, 0, 0]); // parent dim list
    context.add_subdim_relation(SubdimensionRelation::sparse(
        0,
        1,
        SmallVec::from_slice(&[1, 3]),
    ));
    let data = [5.0, 6.0, 7.0, 8.0];
    let got = run_var_view_reduce(
        &context,
        &[Opcode::ViewStarRange {
            dim_idx: 0,
            subdim_relation_id: 0,
        }],
        Opcode::ArraySum {},
        &data,
    );
    assert_eq!(got, 6.0 + 8.0);
}

#[test]
fn dup_view_then_reduce_matches_single() {
    // DupView duplicates the top descriptor; reducing the dup gives the same
    // result as reducing the original (and the original stays on the stack).
    let context = ctx_one_dim(3);
    let data = [2.0, 3.0, 5.0];
    let got = run_var_view_reduce(&context, &[Opcode::DupView {}], Opcode::ArraySum {}, &data);
    assert_eq!(got, 10.0);
    // The duplicate must leave the stack balanced for the trailing PopView;
    // a second PopView would underflow, so add one more here to drain the
    // dup and confirm both pops succeed.
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushVarView {
            base_off: 0,
            dim_list_id: 0,
        },
        Opcode::DupView {},
        Opcode::ArraySum {},
        Opcode::PopView {}, // pop dup
        Opcode::PopView {}, // pop original
    ];
    let got2 = run(&bc(vec![], code), &ctx, true, 0, &seed_run(0, &data), None);
    assert_eq!(got2, 10.0);
}

// ── Task 2: each reducer vs an explicit VM-mirrored oracle ────────────

/// Sum/Max/Min/Mean/Stddev/Size oracle over a contiguous element slice,
/// mirroring the VM's per-reducer arms (`vm.rs:2216-2309`) exactly.
fn reducer_oracle(op: &Opcode, elems: &[f64]) -> f64 {
    let size = elems.len();
    match op {
        Opcode::ArraySum {} => elems.iter().sum(),
        Opcode::ArraySize {} => size as f64,
        _ if size == 0 => f64::NAN,
        Opcode::ArrayMax {} => elems
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, |a, v| if v > a { v } else { a }),
        Opcode::ArrayMin {} => elems
            .iter()
            .copied()
            .fold(f64::INFINITY, |a, v| if v < a { v } else { a }),
        Opcode::ArrayMean {} => elems.iter().sum::<f64>() / size as f64,
        Opcode::ArrayStddev {} => {
            let mean = elems.iter().sum::<f64>() / size as f64;
            let var = elems.iter().map(|v| (v - mean).powf(2.0)).sum::<f64>() / size as f64;
            var.sqrt()
        }
        _ => unreachable!(),
    }
}

fn assert_reducer_matches(op: Opcode, elems: &[f64]) {
    // A bare contiguous 1-D static view over the data.
    let data: Vec<f64> = elems.to_vec();
    let view = dense_view(0, &[elems.len() as u16]);
    let got = run_static_reduce(view, op, &data);
    let want = reducer_oracle(&op, elems);
    if want.is_nan() {
        assert!(got.is_nan(), "{}: expected NaN, got {got}", op.name());
    } else {
        assert!(
            (got - want).abs() < 1e-12,
            "{}: got {got}, want {want}",
            op.name()
        );
    }
}

#[test]
fn reducer_sum_matches_vm() {
    assert_reducer_matches(Opcode::ArraySum {}, &[1.0, 2.0, 3.0, 4.5]);
}

#[test]
fn reducer_max_matches_vm() {
    assert_reducer_matches(Opcode::ArrayMax {}, &[3.0, -1.0, 7.5, 2.0]);
    // Negative-only set: max stays negative (init NEG_INFINITY never wins).
    assert_reducer_matches(Opcode::ArrayMax {}, &[-5.0, -2.0, -9.0]);
}

#[test]
fn reducer_min_matches_vm() {
    assert_reducer_matches(Opcode::ArrayMin {}, &[3.0, -1.0, 7.5, 2.0]);
    assert_reducer_matches(Opcode::ArrayMin {}, &[5.0, 2.0, 9.0]);
}

#[test]
fn reducer_mean_matches_vm() {
    assert_reducer_matches(Opcode::ArrayMean {}, &[2.0, 4.0, 6.0]);
    assert_reducer_matches(Opcode::ArrayMean {}, &[1.0, 2.0]);
}

#[test]
fn reducer_stddev_matches_vm_population_variance() {
    // Population variance (divisor N): for [2,4,4,4,5,5,7,9] the population
    // stddev is exactly 2.0 -- a value check, not just parity, pinning the
    // divisor-N (not N-1) choice that matches `vm.rs::ArrayStddev`.
    let elems = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
    assert_reducer_matches(Opcode::ArrayStddev {}, &elems);
    let view = dense_view(0, &[elems.len() as u16]);
    let got = run_static_reduce(view, Opcode::ArrayStddev {}, &elems);
    assert!(
        (got - 2.0).abs() < 1e-12,
        "population stddev should be 2.0, got {got}"
    );
}

#[test]
fn reducer_size_matches_vm() {
    assert_reducer_matches(Opcode::ArraySize {}, &[1.0, 2.0, 3.0]);
}

#[test]
fn reducer_size_multidim_is_product() {
    // SIZE over a 2x3 view is 6, regardless of the data.
    let data = [0.0; 6];
    let view = dense_view(0, &[2, 3]);
    let got = run_static_reduce(view, Opcode::ArraySize {}, &data);
    assert_eq!(got, 6.0);
}

// ── Task 2: empty-but-valid view asymmetry (AC1.5) ────────────────────

/// An empty-but-valid view: a `[start:start)` range collapses dim 0 to size
/// 0 (`apply_range_checked`), valid with zero elements. Built as a static
/// view with a zero-size dimension.
fn empty_static_view() -> StaticArrayView {
    StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[0]),
        strides: SmallVec::from_slice(&[1]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0]),
    }
}

#[test]
fn empty_valid_view_sum_is_zero() {
    // ArraySum over an empty-but-valid view is the additive identity 0.0
    // (`vm.rs:2216`), NOT NaN.
    let got = run_static_reduce(empty_static_view(), Opcode::ArraySum {}, &[1.0]);
    assert_eq!(got, 0.0);
}

#[test]
fn empty_valid_view_max_min_mean_stddev_are_nan() {
    for op in [
        Opcode::ArrayMax {},
        Opcode::ArrayMin {},
        Opcode::ArrayMean {},
        Opcode::ArrayStddev {},
    ] {
        let got = run_static_reduce(empty_static_view(), op, &[1.0]);
        assert!(
            got.is_nan(),
            "{}: empty-but-valid view must be NaN",
            op.name()
        );
    }
}

#[test]
fn empty_valid_view_size_is_zero() {
    let got = run_static_reduce(empty_static_view(), Opcode::ArraySize {}, &[1.0]);
    assert_eq!(got, 0.0);
}

// ── Task 2: invalid view -> NaN for ALL reducers (AC1.5) ──────────────
//
// A static view is always valid (`valid_local` is None), so an invalid view
// is modeled by directly setting `valid_local` to a wasm i32 local seeded to
// 0 -- mirroring what Task 4's out-of-bounds dynamic subscript will produce.
// Every reducer (including ArraySum) must yield NaN, matching `reduce_view`'s
// leading `if !is_valid { return NaN }`.

/// Run a reducer over a contiguous static view whose `valid_local` is forced
/// to an i32 local pre-set to 0 (invalid). The harness function reserves the
/// three Apply f64 scratch locals; we add one i32 local after them for the
/// validity flag and initialize it to 0 in the emitted prologue.
fn run_invalid_view_reduce(reduce: Opcode) -> f64 {
    let mut context = ByteCodeContext::default();
    // Contiguous 1-D view over 3 curr slots; geometry is valid, but the
    // view is flagged invalid.
    let view = dense_view(0, &[3]);
    let view_id = context.add_static_view(view);

    // Build a custom module: the opcode function declares an extra i32 local
    // (index after the standard opcode-fn locals) for the validity flag,
    // seeded to 0. We mark the descriptor invalid by post-processing is out
    // of reach here, so instead emit the program through a small shim that
    // sets `valid_local` on the pushed descriptor.
    //
    // Simpler: emit PushStaticView, then a hand-rolled reduce over a desc
    // with valid_local set, by calling emit_array_reduce directly.
    let ctx = EmitCtx {
        temp_storage_base: TEMP_BASE,
        ctx: &context,
        ..ctx_with_cond_depth(0)
    };

    // The validity i32 local index: it is the first index past every standard
    // opcode-fn local (the scratch f64, the cond i32s, the Apply f64s, and the
    // Phase-6 vector-op f64/i32 scratch blocks), i.e. exactly where the
    // dynamic-subscript "extra i32" locals begin. The shim below pushes a single
    // i32 local at that index for the validity flag.
    let valid_local = extra_i32_local_base(0);

    let mut module = Module::new();
    let helpers = build_helpers();
    let n_helpers = helpers.functions.len() as u32;
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::F64]); // eval -> f64
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
    exports.export("eval", ExportKind::Func, n_helpers);
    exports.export("mem", ExportKind::Memory, 0);
    module.section(&exports);

    let mut code = CodeSection::new();
    for hf in helpers.functions {
        code.function(&hf.body);
    }
    // opcode-fn locals plus one extra i32 for the validity flag.
    let mut locals = opcode_fn_locals(0, 0);
    locals.push((1, ValType::I32));
    let mut func = Function::new(locals);
    // valid_local = 0 (invalid).
    func.instruction(&Instruction::I32Const(0));
    func.instruction(&Instruction::LocalSet(valid_local));
    // Reduce over a desc built from the registered static view, but with its
    // `valid_local` forced to the (zero-seeded) validity flag -- exactly the
    // shape Task 4's out-of-bounds dynamic subscript will produce.
    let mut desc = ViewDesc::from_static(ctx.ctx.get_static_view(view_id).unwrap());
    desc.valid_local = Some(valid_local);
    emit_array_reduce(&reduce, &desc, &ctx, &mut func).expect("reduce lowers");
    func.instruction(&Instruction::End);
    code.function(&func);
    module.section(&code);

    let bytes = module.finish();
    let info = validate(&bytes).expect("invalid-view module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    // Seed the curr slots so a (wrongly) valid read would produce a finite
    // value -- making the NaN assertion meaningful.
    let mem = store
        .instance_export(inst, "mem")
        .unwrap()
        .as_mem()
        .unwrap();
    store.mem_access_mut_slice(mem, |b| {
        for (i, v) in [1.0f64, 2.0, 3.0].iter().enumerate() {
            let a = i * 8;
            b[a..a + 8].copy_from_slice(&v.to_le_bytes());
        }
    });
    let eval = store
        .instance_export(inst, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    store.invoke_simple_typed(eval, (0_i32,)).expect("invoke")
}

#[test]
fn invalid_view_all_reducers_are_nan() {
    // Every reducer over an invalid view is NaN -- including ArraySum, whose
    // empty-but-valid result is 0.0 but whose invalid-view result is NaN.
    for op in [
        Opcode::ArraySum {},
        Opcode::ArrayMax {},
        Opcode::ArrayMin {},
        Opcode::ArrayMean {},
        Opcode::ArrayStddev {},
    ] {
        let got = run_invalid_view_reduce(op);
        assert!(
            got.is_nan(),
            "{}: an invalid view must reduce to NaN, got {got}",
            op.name()
        );
    }
}

#[test]
fn invalid_view_size_is_still_the_size() {
    // ArraySize is defined regardless of validity (`vm.rs:2306` reads
    // `view.size()` with no validity gate), so an invalid 3-element view
    // still reports size 3.
    let got = run_invalid_view_reduce(Opcode::ArraySize {});
    assert_eq!(got, 3.0);
}

// ════════════════════════════════════════════════════════════════════════
// Phase 5 Task 3: iteration loops (BeginIter..EndIter) + broadcast
//
// The body span between `BeginIter` and `NextIterOrJump` is fully unrolled
// over the compile-time `size()`, so each iteration's reads/writes are
// emitted at constant addresses (mirroring the array reducer's unrolled fold
// and the VM element-for-element). These hand-build the canonical codegen
// shape (`PushStaticView(out); BeginIter; PushStaticView(src); <body>;
// NextIterOrJump; EndIter; PopView; ...`) and run it under DLR-FT, reading
// the written temp slots back and comparing to a VM-mirrored oracle.
// ════════════════════════════════════════════════════════════════════════

/// A contiguous temp `StaticArrayView` over `dims` at `temp_id`.
fn temp_view(temp_id: u32, dims: &[u16]) -> StaticArrayView {
    let mut v = dense_view(temp_id, dims);
    v.is_temp = true;
    v
}

/// A contiguous temp `StaticArrayView` carrying explicit `dim_ids` (for the
/// broadcast-matching tests).
fn dense_view_ids(base_off: u32, dims: &[u16], dim_ids: &[u16]) -> StaticArrayView {
    let mut v = dense_view(base_off, dims);
    v.dim_ids = dim_ids.iter().copied().collect();
    v
}

/// Read `count` temp slots (starting at temp slot 0) back after running a
/// temp-writing program. The temp region base is `TEMP_BASE`.
fn run_and_read_temps(
    context: &ByteCodeContext,
    code: Vec<Opcode>,
    literals: Vec<f64>,
    seed: &[(u64, f64)],
    count: usize,
) -> Vec<f64> {
    let ctx = ctx_with_arrays(context);
    let bytes = build_module(&bc(literals, code), &ctx, false, 0);
    let info = validate(&bytes).expect("emitted module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    if !seed.is_empty() {
        let mem = store
            .instance_export(inst, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        store.mem_access_mut_slice(mem, |b| {
            for &(addr, v) in seed {
                let a = addr as usize;
                b[a..a + 8].copy_from_slice(&v.to_le_bytes());
            }
        });
    }
    let eval = store
        .instance_export(inst, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(i32,), ()>(eval, (0_i32,))
        .expect("invoke");
    let mem = store
        .instance_export(inst, "mem")
        .unwrap()
        .as_mem()
        .unwrap();
    store.mem_access_mut_slice(mem, |b| {
        (0..count)
            .map(|i| {
                let a = TEMP_BASE as usize + i * 8;
                f64::from_le_bytes(b[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

#[test]
fn iter_loop_elementwise_writes_temp_like_vm() {
    // out_temp[i] = source[i] * 2 over a 4-element source in curr, written to
    // temp 0. Mirrors the codegen shape: output temp view drives iteration,
    // the source view is pushed inside, read via LoadIterViewAt{1}.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 4); // temp 0 spans 4 slots
    let out_view = context.add_static_view(temp_view(0, &[4]));
    let src_view = context.add_static_view(dense_view(0, &[4]));
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::PushStaticView { view_id: src_view },
        Opcode::LoadIterViewAt { offset: 1 },
        Opcode::LoadConstant { id: 0 },
        Opcode::Op2 { op: Op2::Mul },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -4 },
        Opcode::EndIter {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    // source = [10, 20, 30, 40] in curr slots 0..4.
    let seed = seed_run(0, &[10.0, 20.0, 30.0, 40.0]);
    let temps = run_and_read_temps(&context, code, vec![2.0], &seed, 4);
    assert_eq!(temps, vec![20.0, 40.0, 60.0, 80.0]);
}

#[test]
fn iter_loop_load_iter_element_reads_captured_view() {
    // out_temp[i] = iter_view[i] (the captured view *is* the iteration view).
    // Here the captured view is the OUTPUT temp itself, so seed the temp and
    // copy it to itself -- a degenerate but faithful LoadIterElement check.
    // Use a separate source temp captured as the iter view instead: push a
    // source temp view, BeginIter captures it, LoadIterElement reads it, and
    // StoreIterElement writes the *same* temp's slots (write_temp == source).
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 3);
    let src = context.add_static_view(temp_view(0, &[3]));
    let code = vec![
        Opcode::PushStaticView { view_id: src },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::LoadIterElement {},
        Opcode::LoadConstant { id: 0 },
        Opcode::Op2 { op: Op2::Add },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -4 },
        Opcode::EndIter {},
        Opcode::PopView {},
    ];
    // temp 0 = [1, 2, 3]; each += 5 in place -> [6, 7, 8].
    let seed = seed_run(u64::from(TEMP_BASE), &[1.0, 2.0, 3.0]);
    let temps = run_and_read_temps(&context, code, vec![5.0], &seed, 3);
    assert_eq!(temps, vec![6.0, 7.0, 8.0]);
}

#[test]
fn iter_loop_load_iter_temp_element_reads_temp() {
    // out_temp1[i] = temp0[i] + 100, reading temp0 via LoadIterTempElement and
    // writing temp1. temp_offsets = [0, 3]: temp0 in slots 0..3, temp1 in 3..6.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0, 3], 6);
    let out_view = context.add_static_view(temp_view(1, &[3])); // temp 1
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 1,
            has_write_temp: true,
        },
        Opcode::LoadIterTempElement { temp_id: 0 },
        Opcode::LoadConstant { id: 0 },
        Opcode::Op2 { op: Op2::Add },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -4 },
        Opcode::EndIter {},
        Opcode::PopView {},
    ];
    // temp0 = [7, 8, 9] in slots 0..3.
    let seed = seed_run(u64::from(TEMP_BASE), &[7.0, 8.0, 9.0]);
    // Read 6 temp slots: temp1 is slots 3..6.
    let temps = run_and_read_temps(&context, code, vec![100.0], &seed, 6);
    assert_eq!(&temps[3..6], &[107.0, 108.0, 109.0]);
}

#[test]
fn iter_loop_broadcast_smaller_source_matches_vm() {
    // out_temp[A,B] = mat[A,B] + vec[A]: the iteration view is 2-D [A(2),B(3)]
    // (dim_ids [0,1]); `vec` is 1-D [A(2)] (dim_id 0), broadcast along B. This
    // exercises the `LoadIterViewAt` broadcast path (source dims != iter
    // dims), which production codegen does not currently emit but the VM
    // supports. Cross-checked element-for-element against the VM's broadcast.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 6); // out temp [2,3]
    // Two indexed dims so match_dimensions_two_pass can resolve is_indexed.
    let na = context.intern_name("A");
    context.add_dimension(DimensionInfo::indexed(na, 2)); // id 0
    let nb = context.intern_name("B");
    context.add_dimension(DimensionInfo::indexed(nb, 3)); // id 1

    let out_view = context.add_static_view({
        let mut v = temp_view(0, &[2, 3]);
        v.dim_ids = SmallVec::from_slice(&[0, 1]);
        v
    });
    // mat in curr slots 0..6 (dims [2,3], dim_ids [0,1]).
    let mat = context.add_static_view(dense_view_ids(0, &[2, 3], &[0, 1]));
    // vec in curr slots 6..8 (dims [2], dim_id 0).
    let vec_v = context.add_static_view(dense_view_ids(6, &[2], &[0]));
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::PushStaticView { view_id: mat }, // offset 2 after vec is pushed
        Opcode::PushStaticView { view_id: vec_v }, // offset 1
        Opcode::LoadIterViewAt { offset: 2 },    // mat[A,B]
        Opcode::LoadIterViewAt { offset: 1 },    // vec[A] broadcast over B
        Opcode::Op2 { op: Op2::Add },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -5 },
        Opcode::EndIter {},
        Opcode::PopView {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    // mat[a,b] = a*10 + b -> [0,1,2, 10,11,12]; vec[a] = a -> [0, 1].
    let mut seed = seed_run(0, &[0.0, 1.0, 2.0, 10.0, 11.0, 12.0]);
    seed.extend(seed_run(6 * 8, &[0.0, 1.0]));
    let temps = run_and_read_temps(&context, code, vec![], &seed, 6);
    // out[a,b] = mat[a,b] + vec[a].
    let expected = [
        0.0 + 0.0,
        1.0 + 0.0,
        2.0 + 0.0,
        10.0 + 1.0,
        11.0 + 1.0,
        12.0 + 1.0,
    ];
    assert_eq!(temps, expected);
}

#[test]
fn iter_loop_smaller_source_same_shape_writes_nan() {
    // The iteration is over 4 elements but the source view (same dim_ids) has
    // only 3: the VM's `LoadIterViewTop`/`LoadIterViewAt` fast path returns
    // NaN past the source size (`vm.rs:1972`). Element 3 must be NaN.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 4);
    let out_view = context.add_static_view(temp_view(0, &[4]));
    let src = context.add_static_view(dense_view(0, &[3])); // shorter
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::PushStaticView { view_id: src },
        Opcode::LoadIterViewAt { offset: 1 },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -3 },
        Opcode::EndIter {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    let seed = seed_run(0, &[5.0, 6.0, 7.0]);
    let temps = run_and_read_temps(&context, code, vec![], &seed, 4);
    assert_eq!(&temps[0..3], &[5.0, 6.0, 7.0]);
    assert!(
        temps[3].is_nan(),
        "element past the source size must be NaN"
    );
}

#[test]
fn iter_loop_then_reduce_dotprod_matches_vm() {
    // The full SUM(a[*]*b[*]) shape: hoist a[i]*b[i] into a temp via BeginIter,
    // then ArraySum the temp. a in curr 0..4, b in curr 4..8, temp 0.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 4);
    let out_view = context.add_static_view(temp_view(0, &[4]));
    let a = context.add_static_view(dense_view(0, &[4]));
    let b = context.add_static_view(dense_view(4, &[4]));
    let temp_read = context.add_static_view(temp_view(0, &[4]));
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::PushStaticView { view_id: a }, // offset 2 after b
        Opcode::PushStaticView { view_id: b }, // offset 1
        Opcode::LoadIterViewAt { offset: 2 },
        Opcode::LoadIterViewAt { offset: 1 },
        Opcode::Op2 { op: Op2::Mul },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -5 },
        Opcode::EndIter {},
        Opcode::PopView {},
        Opcode::PopView {},
        Opcode::PopView {},
        Opcode::PushStaticView { view_id: temp_read },
        Opcode::ArraySum {},
        Opcode::PopView {},
    ];
    // a = [1,2,3,4], b = [10,20,30,40] -> dot = 10+40+90+160 = 300.
    let mut seed = seed_run(0, &[1.0, 2.0, 3.0, 4.0]);
    seed.extend(seed_run(4 * 8, &[10.0, 20.0, 30.0, 40.0]));
    let ctx = ctx_with_arrays(&context);
    let got = run(&bc(vec![], code), &ctx, true, 0, &seed, None);
    assert_eq!(got, 300.0);
}

#[test]
fn iter_loop_zero_size_writes_nothing() {
    // An empty iteration view (size 0): the unroller emits zero body copies,
    // so the temp keeps its seeded value (no write). A trailing reducer over
    // the empty output is 0 for SUM.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 1);
    let out_view = context.add_static_view({
        let mut v = temp_view(0, &[0]); // zero-size dim
        v.dims = SmallVec::from_slice(&[0]);
        v
    });
    let code = vec![
        Opcode::PushStaticView { view_id: out_view },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::LoadIterElement {},
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -2 },
        Opcode::EndIter {},
        Opcode::PopView {},
    ];
    // Seed temp slot 0 with a sentinel; the empty loop must not touch it.
    let seed = seed_run(u64::from(TEMP_BASE), &[42.0]);
    let temps = run_and_read_temps(&context, code, vec![], &seed, 1);
    assert_eq!(temps, vec![42.0], "an empty iteration writes nothing");
}

// ── Broadcast iteration family (BeginBroadcastIter..EndBroadcastIter) ──
//
// Not emitted by current codegen, but lowered for completeness and pinned
// against the VM's `BeginBroadcastIter`/`LoadBroadcastElement` arms
// (`vm.rs:2314-2421`) here. The result geometry is the union of the source
// dim_ids; a 2-D and a 1-D source broadcast into the 2-D result.

#[test]
fn broadcast_iter_unions_dims_like_vm() {
    // dest[A,B] = mat[A,B] * vec[A]: BeginBroadcastIter with two sources
    // (mat 2-D dim_ids [0,1], vec 1-D dim_id 0). The result unions to
    // dim_ids [0,1] (dims [2,3]); vec broadcasts along B.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 6);
    let na = context.intern_name("A");
    context.add_dimension(DimensionInfo::indexed(na, 2)); // id 0
    let nb = context.intern_name("B");
    context.add_dimension(DimensionInfo::indexed(nb, 3)); // id 1
    let mat = context.add_static_view(dense_view_ids(0, &[2, 3], &[0, 1]));
    let vec_v = context.add_static_view(dense_view_ids(6, &[2], &[0]));
    let code = vec![
        // Push the two sources (deepest-first): mat then vec.
        Opcode::PushStaticView { view_id: mat },
        Opcode::PushStaticView { view_id: vec_v },
        Opcode::BeginBroadcastIter {
            n_sources: 2,
            dest_temp_id: 0,
        },
        Opcode::LoadBroadcastElement { source_idx: 0 }, // mat
        Opcode::LoadBroadcastElement { source_idx: 1 }, // vec
        Opcode::Op2 { op: Op2::Mul },
        Opcode::StoreBroadcastElement {},
        Opcode::NextBroadcastOrJump { jump_back: -4 },
        Opcode::EndBroadcastIter {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    // mat[a,b] = a*10 + b -> [0,1,2, 10,11,12]; vec[a] = a+1 -> [1, 2].
    let mut seed = seed_run(0, &[0.0, 1.0, 2.0, 10.0, 11.0, 12.0]);
    seed.extend(seed_run(6 * 8, &[1.0, 2.0]));
    let temps = run_and_read_temps(&context, code, vec![], &seed, 6);
    // dest[a,b] = mat[a,b] * vec[a].
    let expected = [
        0.0 * 1.0,
        1.0 * 1.0,
        2.0 * 1.0,
        10.0 * 2.0,
        11.0 * 2.0,
        12.0 * 2.0,
    ];
    assert_eq!(temps, expected);
}

// ════════════════════════════════════════════════════════════════════════
// Phase 5 Task 4: dynamic subscripts + OOB->NaN
//
// The legacy scalar subscript (`PushSubscriptIndex` / `LoadSubscript`) and
// the view-stack dynamic subscript (`ViewSubscriptDynamic`) both carry a
// runtime offset + validity flag in fresh i32 wasm locals (reserved by
// `count_extra_i32_locals`). An out-of-bounds index clears the validity
// flag, so the read yields NaN -- matching the VM (`vm.rs:1341-1366` for the
// legacy path; `reduce_view`'s `if !is_valid { NaN }` for the view path).
// ════════════════════════════════════════════════════════════════════════

/// Run `code` (with `count_extra_i32_locals` reserved) returning the f64
/// result, with `curr` seeded from `data` (slot 0 = byte 0). The literal pool
/// holds the runtime index value(s).
fn run_dyn(code: Vec<Opcode>, literals: Vec<f64>, data: &[f64]) -> f64 {
    let context = ByteCodeContext::default();
    let ctx = ctx_with_arrays(&context);
    run(&bc(literals, code), &ctx, true, 0, &seed_run(0, data), None)
}

#[test]
fn legacy_subscript_1d_in_range_matches_vm() {
    // arr[idx] (idx 1-based) over a 4-element array in curr slots 0..4.
    // idx = 3 (1-based) -> 0-based 2 -> data[2].
    let data = [10.0, 20.0, 30.0, 40.0];
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // idx = 3.0
        Opcode::PushSubscriptIndex { bounds: 4 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert_eq!(run_dyn(code, vec![3.0], &data), 30.0);
}

#[test]
fn legacy_subscript_oob_is_nan() {
    let data = [10.0, 20.0, 30.0, 40.0];
    // idx = 5 > bounds 4 -> invalid -> NaN.
    let high = vec![
        Opcode::LoadConstant { id: 0 },
        Opcode::PushSubscriptIndex { bounds: 4 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert!(
        run_dyn(high, vec![5.0], &data).is_nan(),
        "idx > bounds -> NaN"
    );
    // idx = 0 is invalid in 1-based indexing -> NaN.
    let zero = vec![
        Opcode::LoadConstant { id: 0 },
        Opcode::PushSubscriptIndex { bounds: 4 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert!(run_dyn(zero, vec![0.0], &data).is_nan(), "idx 0 -> NaN");
}

#[test]
fn legacy_subscript_off_shifts_base_like_vm() {
    // LoadSubscript reads curr[module_off + off + flat]; with off=2 the base
    // shifts by 2 slots. arr starts at slot 2; idx=2 (1-based) -> slot 3.
    let data = [99.0, 99.0, 100.0, 200.0, 300.0];
    let code = vec![
        Opcode::LoadConstant { id: 0 },
        Opcode::PushSubscriptIndex { bounds: 3 },
        Opcode::LoadSubscript { off: 2 },
    ];
    assert_eq!(run_dyn(code, vec![2.0], &data), 200.0);
}

#[test]
fn legacy_subscript_2d_fold_matches_vm() {
    // arr[i, j] over a [2,3] row-major array in curr slots 0..6. The VM folds
    // index = i0*bounds1 + i1 (the running index times the current bound plus
    // the current index). i=2 (1-based -> 0-based 1), j=3 (1-based -> 0-based
    // 2): flat = 1*3 + 2 = 5 -> data[5].
    let data = [0.0, 1.0, 2.0, 10.0, 11.0, 12.0];
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // i = 2.0
        Opcode::PushSubscriptIndex { bounds: 2 },
        Opcode::LoadConstant { id: 1 }, // j = 3.0
        Opcode::PushSubscriptIndex { bounds: 3 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert_eq!(run_dyn(code, vec![2.0, 3.0], &data), 12.0);
}

#[test]
fn legacy_subscript_2d_oob_in_either_index_is_nan() {
    let data = [0.0, 1.0, 2.0, 10.0, 11.0, 12.0];
    // Second index out of bounds (j=4 > 3) -> NaN even though i is valid.
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // i = 1
        Opcode::PushSubscriptIndex { bounds: 2 },
        Opcode::LoadConstant { id: 1 }, // j = 4 (oob)
        Opcode::PushSubscriptIndex { bounds: 3 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert!(run_dyn(code, vec![1.0, 4.0], &data).is_nan());
}

#[test]
fn legacy_subscript_floors_fractional_index() {
    // The VM does `stack.pop().floor() as u16`; idx 2.9 -> 1-based 2 -> slot 1.
    let data = [10.0, 20.0, 30.0];
    let code = vec![
        Opcode::LoadConstant { id: 0 },
        Opcode::PushSubscriptIndex { bounds: 3 },
        Opcode::LoadSubscript { off: 0 },
    ];
    assert_eq!(run_dyn(code, vec![2.9], &data), 20.0);
}

/// Build a 1-D `PushVarViewDirect` over `dim` slots, apply a dynamic subscript
/// at dim 0 from a constant index, and `ArraySum` the resulting (scalar) view
/// -- the `ViewSubscriptDynamic` end-to-end shape, runnable in isolation.
fn run_view_dyn_subscript(dim: u16, index: f64, data: &[f64]) -> f64 {
    let mut context = ByteCodeContext::default();
    // PushVarViewDirect resolves dims from a dim-list of raw sizes.
    context.add_dim_list(1, [dim, 0, 0, 0]);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushVarViewDirect {
            base_off: 0,
            dim_list_id: 0,
        },
        Opcode::LoadConstant { id: 0 }, // dynamic index
        Opcode::ViewSubscriptDynamic { dim_idx: 0 },
        Opcode::ArraySum {}, // sum of the 1-element view (or NaN if invalid)
        Opcode::PopView {},
    ];
    run(
        &bc(vec![index], code),
        &ctx,
        true,
        0,
        &seed_run(0, data),
        None,
    )
}

#[test]
fn view_subscript_dynamic_in_range_reads_element() {
    // arr[idx] reduced: idx = 3 (1-based) -> data[2]; SUM of the 1-element
    // view is that element.
    let data = [10.0, 20.0, 30.0, 40.0];
    assert_eq!(run_view_dyn_subscript(4, 3.0, &data), 30.0);
}

#[test]
fn view_subscript_dynamic_oob_is_nan() {
    let data = [10.0, 20.0, 30.0, 40.0];
    // idx = 5 > dim 4 -> view invalid -> reducer (even SUM) yields NaN.
    assert!(
        run_view_dyn_subscript(4, 5.0, &data).is_nan(),
        "idx > dim -> invalid view -> NaN"
    );
    // idx = 0 invalid (1-based) -> NaN.
    assert!(
        run_view_dyn_subscript(4, 0.0, &data).is_nan(),
        "idx 0 -> invalid view -> NaN"
    );
}

#[test]
fn view_subscript_dynamic_offset_picks_right_element() {
    // Sweep the in-range indices: each picks the matching element.
    let data = [5.0, 6.0, 7.0, 8.0, 9.0];
    for (idx_1based, expected) in [(1, 5.0), (2, 6.0), (3, 7.0), (4, 8.0), (5, 9.0)] {
        assert_eq!(
            run_view_dyn_subscript(5, idx_1based as f64, &data),
            expected,
            "arr[{idx_1based}] (1-based)"
        );
    }
}

// ── End-to-end: a runtime-OOB dynamic subscript feeding a real reducer ────
//
// The white-box `run_invalid_view_reduce` above hand-forces `valid_local`;
// this composes the genuine codegen shape -- `mat[oob_row, *]` where `row` is
// a runtime out-of-range index -- so the invalid-view NaN flows from a real
// `ViewSubscriptDynamic` through `emit_array_reduce`'s validity gate, over a
// multi-element (non-degenerate) row, exactly as a model would produce it.

/// Build a 2-D `mat[rows][cols]` view via `PushVarViewDirect`, dynamically
/// subscript dim 0 with a runtime `row_1based` index (leaving a `cols`-element
/// row view), and reduce that row. The row is invalid iff `row_1based` is out
/// of `1..=rows`. `data` seeds the row-major curr slab (rows*cols slots).
fn run_view_dyn_row_reduce(
    rows: u16,
    cols: u16,
    row_1based: f64,
    reduce: Opcode,
    data: &[f64],
) -> f64 {
    let mut context = ByteCodeContext::default();
    context.add_dim_list(2, [rows, cols, 0, 0]);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushVarViewDirect {
            base_off: 0,
            dim_list_id: 0,
        },
        Opcode::LoadConstant { id: 0 }, // runtime row index (1-based)
        Opcode::ViewSubscriptDynamic { dim_idx: 0 },
        reduce,
        Opcode::PopView {},
    ];
    run(
        &bc(vec![row_1based], code),
        &ctx,
        true,
        0,
        &seed_run(0, data),
        None,
    )
}

#[test]
fn view_dyn_oob_row_makes_every_reducer_nan() {
    // A 3x4 matrix; row index 5 is out of range (rows = 3). The subscripted
    // view spans a real 4-element row, but its validity flag is 0, so EVERY
    // reducer -- including ArraySum, whose empty-but-valid result is 0.0 --
    // must yield NaN, matching `reduce_view`'s leading `if !is_valid`.
    let data: Vec<f64> = (0..12).map(|i| i as f64).collect();
    for op in [
        Opcode::ArraySum {},
        Opcode::ArrayMax {},
        Opcode::ArrayMin {},
        Opcode::ArrayMean {},
        Opcode::ArrayStddev {},
    ] {
        let got = run_view_dyn_row_reduce(3, 4, 5.0, op, &data);
        assert!(
            got.is_nan(),
            "{}: an out-of-range dynamic row subscript must reduce to NaN, got {got}",
            op.name()
        );
    }
    // ArraySize is defined regardless of validity: a 4-wide row reports 4.
    assert_eq!(
        run_view_dyn_row_reduce(3, 4, 5.0, Opcode::ArraySize {}, &data),
        4.0
    );
}

#[test]
fn view_dyn_in_range_row_reduces_like_vm() {
    // The same shape with an in-range row index reduces the real row, so the
    // NaN above is genuinely the validity gate, not a broken reducer. Row 2
    // (1-based) of a 3x4 row-major matrix is slots 4..8 -> [4,5,6,7].
    let data: Vec<f64> = (0..12).map(|i| i as f64).collect();
    let row = [4.0f64, 5.0, 6.0, 7.0];
    let sum: f64 = row.iter().sum();
    let mean = sum / row.len() as f64;
    let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / row.len() as f64;
    assert_eq!(
        run_view_dyn_row_reduce(3, 4, 2.0, Opcode::ArraySum {}, &data),
        sum
    );
    assert_eq!(
        run_view_dyn_row_reduce(3, 4, 2.0, Opcode::ArrayMax {}, &data),
        7.0
    );
    assert_eq!(
        run_view_dyn_row_reduce(3, 4, 2.0, Opcode::ArrayMin {}, &data),
        4.0
    );
    assert_eq!(
        run_view_dyn_row_reduce(3, 4, 2.0, Opcode::ArrayMean {}, &data),
        mean
    );
    assert!(
        (run_view_dyn_row_reduce(3, 4, 2.0, Opcode::ArrayStddev {}, &data) - var.sqrt()).abs()
            < 1e-12
    );
}

// ════════════════════════════════════════════════════════════════════════
// IMPORTANT (review feedback): full-unrolling has a documented size cap.
//
// Reducers, `BeginIter`, and `BeginBroadcastIter` all unroll fully at compile
// time. `EmitState::charge_unroll` bounds the cumulative element count per
// function at `MAX_UNROLL_UNITS`, returning `Unsupported` (so the model falls
// back to the VM) before any oversized body is emitted. These check the cap
// directly via `emit_bytecode`, asserting an over-budget program is rejected
// WITHOUT materializing a giant function, and an under-budget one still emits.
// ════════════════════════════════════════════════════════════════════════

/// Lower `bc` into a throwaway function, returning the lowering result. Used
/// to assert that an over-budget program is rejected at emit time without
/// running (or even finishing building) the module.
fn lower_only(bc: &ByteCode, ctx: &EmitCtx) -> Result<Function, WasmGenError> {
    let mut func = Function::new(opcode_fn_locals(0, count_extra_i32_locals(bc)));
    emit_bytecode(bc, ctx, &mut func)?;
    func.instruction(&Instruction::End);
    Ok(func)
}

#[test]
fn reducer_over_view_exceeding_cap_is_unsupported() {
    // A single static view whose element count exceeds MAX_UNROLL_UNITS. Two
    // u16 dims (300 x 300 = 90_000 > 65_536) overflow the budget; the cap is
    // checked before the fold, so lowering returns Unsupported with no
    // emitted body. The fixture itself is tiny -- proving we reject rather
    // than emit a multi-megabyte function.
    let mut context = ByteCodeContext::default();
    let view_id = context.add_static_view(dense_view(0, &[300, 300]));
    assert!(dense_view(0, &[300, 300]).to_runtime_view().size() > MAX_UNROLL_UNITS);
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id },
        Opcode::ArraySum {},
        Opcode::PopView {},
    ];
    match lower_only(&bc(vec![], code), &ctx) {
        Err(WasmGenError::Unsupported(msg)) => assert!(
            msg.contains("unrolling exceeds"),
            "expected the unroll-budget message, got: {msg}"
        ),
        Ok(_) => panic!("a reducer over a view larger than the cap must be Unsupported"),
    }
}

#[test]
fn iteration_over_view_exceeding_cap_is_unsupported() {
    // A `BeginIter` whose iteration count exceeds the cap is rejected before
    // the body is re-emitted even once past the budget. Geometry: a 300x300
    // temp written elementwise from a same-shaped source.
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], 300 * 300);
    let out = context.add_static_view(temp_view(0, &[300, 300]));
    let src = context.add_static_view(dense_view(0, &[300, 300]));
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id: out },
        Opcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: true,
        },
        Opcode::PushStaticView { view_id: src },
        Opcode::LoadIterViewAt { offset: 1 },
        Opcode::StoreIterElement {},
        Opcode::NextIterOrJump { jump_back: -3 },
        Opcode::EndIter {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    match lower_only(&bc(vec![], code), &ctx) {
        Err(WasmGenError::Unsupported(msg)) => assert!(
            msg.contains("unrolling exceeds"),
            "expected the unroll-budget message, got: {msg}"
        ),
        Ok(_) => panic!("an iteration larger than the cap must be Unsupported"),
    }
}

#[test]
fn array_size_over_huge_view_is_free() {
    // ArraySize emits no element reads (`size() as f64`), so it must NOT be
    // charged against the unroll budget: a view far larger than the cap still
    // reports its size and lowers fine.
    let mut context = ByteCodeContext::default();
    let view_id = context.add_static_view(dense_view(0, &[300, 300]));
    let ctx = ctx_with_arrays(&context);
    let code = vec![
        Opcode::PushStaticView { view_id },
        Opcode::ArraySize {},
        Opcode::PopView {},
    ];
    assert!(
        lower_only(&bc(vec![], code), &ctx).is_ok(),
        "ArraySize does no element reads and must not be capped"
    );
}

#[test]
fn reducer_just_under_cap_compiles_and_matches_vm() {
    // A view sized just under the cap still lowers and runs to VM parity. We
    // keep the fixture small/fast (a 64-element view) but assert the budget
    // accounting admits it: 64 << MAX_UNROLL_UNITS. (The full corpus of small
    // arrayed reducer tests above is the broad just-under-cap parity check;
    // this pins the boundary intent.)
    let data: Vec<f64> = (0..64).map(|i| (i as f64) * 0.5).collect();
    let view = dense_view(0, &[64]);
    assert!(view.to_runtime_view().size() <= MAX_UNROLL_UNITS);
    let got = run_static_reduce(view.clone(), Opcode::ArraySum {}, &data);
    assert_eq!(got, vm_sum(&view, &data));
}

#[test]
fn unroll_cap_has_headroom_over_realistic_arrays() {
    // The cap must be generous enough for real SD models. The test corpus's
    // largest single dimension is 9; even a region x sector x cohort nest is
    // ~10^3 elements. A compile-time assert pins that the cap clears a
    // deliberately roomy 10^4 with margin, documenting that legitimate models
    // never trip it.
    const _: () = assert!(
        MAX_UNROLL_UNITS >= 10_000,
        "the unroll cap must leave ample headroom for realistic arrayed models"
    );
}

// ════════════════════════════════════════════════════════════════════════
// Phase 6 Task 1: VECTOR SELECT + VECTOR ELM MAP
//
// `VectorSelect` reduces two views (a selector mask + an expression array) to
// ONE scalar pushed on the stack. `VectorElmMap` maps a source array through a
// per-element offset array into a `write_temp_id` temp region. Both are run
// under DLR-FT and cross-checked against the VM: VectorSelect against a faithful
// oracle of the `vm.rs:2444-2502` arm, VectorElmMap against the sibling
// `crate::vm_vector_elm_map::vector_elm_map` function directly.
// ════════════════════════════════════════════════════════════════════════

/// The VM `VectorSelect` oracle (mirroring `vm.rs:2444-2502`): zip the two views
/// to the shorter size, collect `expr` where `is_truthy(sel)`, then dispatch the
/// action (1=min, 2=mean, 3=max, 4=product, else sum) with the empty-selection
/// fallback to `max_value`.
fn vm_vector_select_oracle(
    sel_view: &StaticArrayView,
    expr_view: &StaticArrayView,
    sel_data: &[f64],
    expr_data: &[f64],
    max_value: f64,
    action: i32,
) -> f64 {
    let sel_rv = sel_view.to_runtime_view();
    let expr_rv = expr_view.to_runtime_view();
    let size = sel_rv.size().min(expr_rv.size());
    let mut selected: Vec<f64> = Vec::new();
    let mut sel_idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; sel_rv.dims.len()];
    let mut expr_idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; expr_rv.dims.len()];
    for _ in 0..size {
        let sel_off = sel_rv.flat_offset(&sel_idx);
        let sel_val = sel_data[sel_rv.base_off as usize + sel_off];
        if crate::vm::is_truthy(sel_val) {
            let expr_off = expr_rv.flat_offset(&expr_idx);
            selected.push(expr_data[expr_rv.base_off as usize + expr_off]);
        }
        crate::vm::increment_indices(&mut sel_idx, &sel_rv.dims);
        crate::vm::increment_indices(&mut expr_idx, &expr_rv.dims);
    }
    if selected.is_empty() {
        max_value
    } else {
        match action {
            1 => selected.iter().cloned().fold(f64::INFINITY, f64::min),
            2 => selected.iter().sum::<f64>() / selected.len() as f64,
            3 => selected.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            4 => selected.iter().product(),
            _ => selected.iter().sum(),
        }
    }
}

/// Run `PushStaticView(sel); PushStaticView(expr); VectorSelect` over a `curr`
/// slab. The two views are pushed sel-then-expr so `expr_view = top`,
/// `sel_view = top-1` (matching the VM). `max_value`/`action` are pushed as the
/// two operands beneath `VectorSelect` (the VM pops `action` then `max_value`).
#[allow(clippy::too_many_arguments)]
fn run_vector_select(
    sel_view: StaticArrayView,
    expr_view: StaticArrayView,
    sel_base: u32,
    expr_base: u32,
    data: &[f64],
    max_value: f64,
    action: f64,
) -> f64 {
    let mut context = ByteCodeContext::default();
    let sel_id = context.add_static_view(sel_view);
    let expr_id = context.add_static_view(expr_view);
    let ctx = ctx_with_arrays(&context);
    let _ = (sel_base, expr_base);
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // max_value (pushed first)
        Opcode::LoadConstant { id: 1 }, // action (pushed second, on top)
        Opcode::PushStaticView { view_id: sel_id },
        Opcode::PushStaticView { view_id: expr_id },
        Opcode::VectorSelect {},
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    run(
        &bc(vec![max_value, action], code),
        &ctx,
        true,
        0,
        &seed_run(0, data),
        None,
    )
}

/// Assert the emitted `VectorSelect` matches the VM oracle for `action`, on the
/// shared `sel`/`expr` fixture seeded from `data` (sel slots 0..4, expr 4..8).
fn assert_vector_select_matches(action: f64, max_value: f64) {
    let sel = dense_view(0, &[4]);
    let expr = dense_view(4, &[4]);
    let data = [1.0, 0.0, 1.0, 1.0, 10.0, 20.0, 30.0, 40.0];
    let got = run_vector_select(sel.clone(), expr.clone(), 0, 4, &data, max_value, action);
    let want = vm_vector_select_oracle(&sel, &expr, &data, &data, max_value, action.round() as i32);
    if want.is_nan() {
        assert!(got.is_nan(), "action {action}: expected NaN, got {got}");
    } else {
        assert_eq!(got, want, "action {action}: got {got}, want {want}");
    }
}

#[test]
fn vector_select_sum_matches_vm() {
    // sel = [1, 0, 1, 1] (mask), expr = [10, 20, 30, 40], action 5 (sum).
    // Selected = [10, 30, 40] -> 80.
    assert_vector_select_matches(5.0, -1.0);
    let sel = dense_view(0, &[4]);
    let expr = dense_view(4, &[4]);
    let data = [1.0, 0.0, 1.0, 1.0, 10.0, 20.0, 30.0, 40.0];
    let got = run_vector_select(sel, expr, 0, 4, &data, -1.0, 5.0);
    assert_eq!(got, 80.0);
}

#[test]
fn vector_select_each_action_matches_vm() {
    // 1=min, 2=mean, 3=max, 4=product, and a few "else -> sum" actions. The
    // selected set is [10, 30, 40]: min 10, mean 80/3, max 40, product 12000,
    // sum 80.
    for action in [1.0, 2.0, 3.0, 4.0, 0.0, 5.0, 7.0] {
        assert_vector_select_matches(action, -1.0);
    }
}

#[test]
fn vector_select_empty_selection_returns_max_value() {
    // An all-false mask selects nothing, so the result is `max_value` for every
    // action (the VM's `if selected.is_empty() { max_value }`).
    let sel = dense_view(0, &[4]);
    let expr = dense_view(4, &[4]);
    // Mask all zero.
    let data = [0.0, 0.0, 0.0, 0.0, 10.0, 20.0, 30.0, 40.0];
    for action in [1.0, 2.0, 3.0, 4.0, 5.0] {
        let got = run_vector_select(sel.clone(), expr.clone(), 0, 4, &data, 123.5, action);
        let want = vm_vector_select_oracle(&sel, &expr, &data, &data, 123.5, action.round() as i32);
        assert_eq!(
            got, want,
            "action {action}: empty selection must be max_value"
        );
        assert_eq!(got, 123.5);
    }
}

#[test]
fn vector_select_nan_in_mask_is_truthy_like_vm() {
    // is_truthy(NaN) is true (approx_eq(NaN, 0) is false), so a NaN mask entry
    // SELECTS its expr value, exactly as the VM does. Mask = [NaN, 0, 1]:
    // selects expr[0] and expr[2].
    let sel = dense_view(0, &[3]);
    let expr = dense_view(3, &[3]);
    let data = [f64::NAN, 0.0, 1.0, 100.0, 200.0, 300.0];
    for action in [1.0, 3.0, 5.0] {
        let got = run_vector_select(sel.clone(), expr.clone(), 3, 3, &data, -1.0, action);
        let want = vm_vector_select_oracle(&sel, &expr, &data, &data, -1.0, action.round() as i32);
        assert_eq!(
            got, want,
            "action {action}: NaN mask entry must select its expr"
        );
    }
}

#[test]
fn vector_select_zip_stops_at_shorter_view() {
    // sel has 4 elements, expr has 2: the VM zips to min(4, 2) = 2, so only the
    // first two (sel, expr) pairs are considered. Mask [1, 1, ...] selects
    // expr[0], expr[1]; the trailing sel entries never read a (nonexistent)
    // expr element.
    let sel = dense_view(0, &[4]);
    let expr = dense_view(4, &[2]);
    let data = [1.0, 1.0, 1.0, 1.0, 7.0, 11.0];
    let got = run_vector_select(sel.clone(), expr.clone(), 0, 4, &data, -1.0, 5.0);
    let want = vm_vector_select_oracle(&sel, &expr, &data, &data, -1.0, 5);
    assert_eq!(got, want);
    assert_eq!(got, 18.0, "sum of the first two expr values");
}

#[test]
fn vector_select_nan_expr_value_ignored_by_minmax_like_vm() {
    // A selected expr value of NaN is ignored by min/max (the VM folds with
    // `f64::min`/`f64::max`, which return the non-NaN operand), so wasm `f64.min`/
    // `f64.max` (NaN-propagating) would diverge -- this pins the faithful
    // NaN-ignoring fold. Selected = [10, NaN, 40]: min 10, max 40 (NOT NaN);
    // sum/mean/product DO see the NaN (VM uses `+`/`*`, which propagate).
    let sel = dense_view(0, &[3]);
    let expr = dense_view(3, &[3]);
    let data = [1.0, 1.0, 1.0, 10.0, f64::NAN, 40.0];
    // min and max must be exactly 10 and 40 (NaN ignored).
    assert_eq!(
        run_vector_select(sel.clone(), expr.clone(), 3, 3, &data, -1.0, 1.0),
        10.0
    );
    assert_eq!(
        run_vector_select(sel.clone(), expr.clone(), 3, 3, &data, -1.0, 3.0),
        40.0
    );
    // sum/product propagate the NaN, matching the VM (cross-checked vs oracle).
    for action in [2.0, 4.0, 5.0] {
        assert_vector_select_nan_expr(&sel, &expr, &data, action);
    }
}

fn assert_vector_select_nan_expr(
    sel: &StaticArrayView,
    expr: &StaticArrayView,
    data: &[f64],
    action: f64,
) {
    let got = run_vector_select(sel.clone(), expr.clone(), 3, 3, data, -1.0, action);
    let want = vm_vector_select_oracle(sel, expr, data, data, -1.0, action.round() as i32);
    if want.is_nan() {
        assert!(got.is_nan(), "action {action}: expected NaN, got {got}");
    } else {
        assert_eq!(got, want, "action {action}");
    }
}

// ── VectorElmMap parity vs the sibling VM function ────────────────────────

/// Run `PushStaticView(source); PushStaticView(offset); VectorElmMap` over a
/// `curr` slab seeded from `data`, writing temp 0, and read back `count` temp
/// slots. The source view is pushed first (`top-1`), the offset view second
/// (`top`), matching the VM (`offset_view = top, source_view = top-1`).
fn run_vector_elm_map(
    source: StaticArrayView,
    offset: StaticArrayView,
    full_source_len: u32,
    data: &[f64],
    temp_count: usize,
    temp_slots: usize,
) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], temp_slots);
    let src_id = context.add_static_view(source);
    let off_id = context.add_static_view(offset);
    let code = vec![
        Opcode::PushStaticView { view_id: src_id },
        Opcode::PushStaticView { view_id: off_id },
        Opcode::VectorElmMap {
            write_temp_id: 0,
            full_source_len,
        },
        Opcode::PopView {},
        Opcode::PopView {},
    ];
    run_and_read_temps(&context, code, vec![], &seed_run(0, data), temp_count)
}

/// The VM oracle for `VectorElmMap`: run the sibling
/// `crate::vm_vector_elm_map::vector_elm_map` over `RuntimeView`s built from the
/// same static views, reading `curr` from `data`. Returns the written temp 0
/// slots (`temp_slots` wide).
fn vm_elm_map_oracle(
    source: &StaticArrayView,
    offset: &StaticArrayView,
    full_source_len: u32,
    data: &[f64],
    temp_slots: usize,
) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], temp_slots);
    let mut temp_storage = vec![0.0f64; temp_slots];
    crate::vm_vector_elm_map::vector_elm_map(
        &source.to_runtime_view(),
        &offset.to_runtime_view(),
        0,
        full_source_len,
        data,
        &mut temp_storage,
        &context,
    );
    temp_storage
}

/// Assert the emitted `VectorElmMap` matches the sibling VM function element-for-
/// element over the `offset_view` size (NaN compares as NaN).
fn assert_elm_map_matches(
    source: &StaticArrayView,
    offset: &StaticArrayView,
    full_source_len: u32,
    data: &[f64],
    temp_slots: usize,
) {
    let got = run_vector_elm_map(
        source.clone(),
        offset.clone(),
        full_source_len,
        data,
        temp_slots,
        temp_slots,
    );
    let want = vm_elm_map_oracle(source, offset, full_source_len, data, temp_slots);
    assert_eq!(got.len(), want.len());
    for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
        if w.is_nan() {
            assert!(g.is_nan(), "elm_map slot {i}: expected NaN, got {g}");
        } else {
            assert_eq!(g, w, "elm_map slot {i}: got {g}, want {w}");
        }
    }
}

#[test]
fn vector_elm_map_full_array_in_range_matches_vm() {
    // Full contiguous source [a,b,c,d] in curr slots 0..4; offset [1,3,0,2] in
    // curr slots 4..8 -> result = source[round(offset[i])] = [b, d, a, c].
    let source = dense_view(0, &[4]);
    let offset = dense_view(4, &[4]);
    let data = [10.0, 20.0, 30.0, 40.0, 1.0, 3.0, 0.0, 2.0];
    assert_elm_map_matches(&source, &offset, 4, &data, 4);
    let got = run_vector_elm_map(source, offset, 4, &data, 4, 4);
    assert_eq!(got, vec![20.0, 40.0, 10.0, 30.0]);
}

#[test]
fn vector_elm_map_out_of_range_offset_is_nan() {
    // An offset that lands outside [0, full_source_len) yields NaN (no modulo).
    // Source len 3; offsets [0, 5, -1] -> [source[0], NaN, NaN].
    let source = dense_view(0, &[3]);
    let offset = dense_view(3, &[3]);
    let data = [7.0, 8.0, 9.0, 0.0, 5.0, -1.0];
    assert_elm_map_matches(&source, &offset, 3, &data, 3);
    let got = run_vector_elm_map(source, offset, 3, &data, 3, 3);
    assert_eq!(got[0], 7.0);
    assert!(got[1].is_nan() && got[2].is_nan());
}

#[test]
fn vector_elm_map_nan_offset_is_nan() {
    // A NaN offset yields NaN, regardless of the (would-be) index.
    let source = dense_view(0, &[3]);
    let offset = dense_view(3, &[3]);
    let data = [7.0, 8.0, 9.0, 1.0, f64::NAN, 2.0];
    assert_elm_map_matches(&source, &offset, 3, &data, 3);
    let got = run_vector_elm_map(source, offset, 3, &data, 3, 3);
    assert_eq!(got[0], 8.0);
    assert!(got[1].is_nan());
    assert_eq!(got[2], 9.0);
}

#[test]
fn vector_elm_map_offset_rounds_half_away_like_vm() {
    // The VM rounds the offset with `f64::round` (half away from zero), NOT wasm
    // `f64.nearest` (half to even). Offsets [0.5, 1.5, 2.5] round to [1, 2, 3]
    // (away from zero), not [0, 2, 2] (to even). Cross-checked vs the sibling.
    let source = dense_view(0, &[4]);
    let offset = dense_view(4, &[3]);
    let data = [10.0, 20.0, 30.0, 40.0, 0.5, 1.5, 2.5];
    assert_elm_map_matches(&source, &offset, 4, &data, 3);
    let got = run_vector_elm_map(source, offset, 4, &data, 3, 3);
    // round(0.5)=1 -> source[1]=20; round(1.5)=2 -> 30; round(2.5)=3 -> 40.
    assert_eq!(got, vec![20.0, 30.0, 40.0]);
}

#[test]
fn vector_elm_map_sliced_source_base_i_matches_vm() {
    // A strict-slice source: a 2-D source [DimA(2), DimB(3)] (full storage 6
    // elements in curr 0..6), sliced... here we exercise the carried-axis base_i
    // projection via a source whose remaining dim shares its dim_id with the
    // offset view. Source = matrix[A,B] row-major; offset view is 2-D [A,B] with
    // matching dim_ids, so element (a,b) reads source[base_i(a) + round(off)].
    //
    // Build source as [A(2), B(3)] dim_ids [0,1] over storage [0..6], and offset
    // as [A(2)] dim_id [0] -- but VECTOR ELM MAP needs offset.size() result
    // slots, so use a 2-D offset matching the result. We model the genuine
    // shape: source full storage len 6, source view is the full [2,3], offset
    // [2,3] with the same dim_ids; base_i is 0 (full array) and offset indexes
    // the whole storage. To exercise a NON-zero base_i we instead slice the
    // source to a single row and give the offset that row's dim.
    //
    // Simpler faithful base_i case: source view = row 1 of a [2,3] matrix
    // (offset folds in 3), dim_ids [1] (DimB); offset view [3] dim_id [1]. Then
    // base_i = source.flat_offset([b]) projects DimB, and the result reads
    // storage[3 + round(off)]. full_source_len = 6.
    let mut source = dense_view(0, &[3]); // the sliced row: dims [3]
    source.offset = 3; // row 1 of a [2,3] matrix starts at flat 3
    source.dim_ids = SmallVec::from_slice(&[1]); // DimB
    let mut offset = dense_view(6, &[3]);
    offset.dim_ids = SmallVec::from_slice(&[1]); // DimB, matching the source
    // Storage: matrix rows [r0: 100,101,102][r1: 200,201,202]; offsets [0,1,2].
    let data = [100.0, 101.0, 102.0, 200.0, 201.0, 202.0, 0.0, 1.0, 2.0];
    assert_elm_map_matches(&source, &offset, 6, &data, 3);
    let got = run_vector_elm_map(source, offset, 6, &data, 3, 3);
    // base_i for element b is source.flat_offset([b]) = 3 + b; + round(off[b]):
    //   b=0: 3 + 0 -> storage[3]=200; b=1: 4 + 1 -> storage[5]=202;
    //   b=2: 5 + 2 = 7 -> OOB (>=6) -> NaN.
    assert_eq!(got[0], 200.0);
    assert_eq!(got[1], 202.0);
    assert!(got[2].is_nan());
}

// ── VectorSortOrder / Rank parity vs the VM (stable sort) ─────────────────

/// Run `PushStaticView(input); Vector{SortOrder|Rank}` over a `curr` slab seeded
/// from `data`, writing temp 0, and read back `temp_count` temp slots. The
/// `direction` operand is pushed beneath the op.
fn run_sort_op(
    input: StaticArrayView,
    op: Opcode,
    direction: f64,
    data: &[f64],
    temp_count: usize,
    temp_slots: usize,
) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], temp_slots);
    let in_id = context.add_static_view(input);
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // direction
        Opcode::PushStaticView { view_id: in_id },
        op,
        Opcode::PopView {},
    ];
    run_and_read_temps(
        &context,
        code,
        vec![direction],
        &seed_run(0, data),
        temp_count,
    )
}

/// The VM oracle for `VectorSortOrder`: run the sibling
/// `crate::vm_vector_sort_order::vector_sort_order` over a `RuntimeView`.
fn vm_sort_order_oracle(
    input: &StaticArrayView,
    direction: i32,
    data: &[f64],
    temp_slots: usize,
) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], temp_slots);
    let mut temp_storage = vec![0.0f64; temp_slots];
    crate::vm_vector_sort_order::vector_sort_order(
        &input.to_runtime_view(),
        direction,
        0,
        data,
        &mut temp_storage,
        &context,
    );
    temp_storage
}

/// A faithful local oracle for `Rank` (mirroring `vm.rs:2540-2584`): over the
/// whole view, collect `(value, orig_idx)`, stable sort (asc if direction==1
/// else desc, NaN-as-Equal), write `temp[orig_idx] = rank_0based + 1`.
fn vm_rank_oracle(
    input: &StaticArrayView,
    direction: i32,
    data: &[f64],
    temp_slots: usize,
) -> Vec<f64> {
    let rv = input.to_runtime_view();
    let size = rv.size();
    let mut indexed: Vec<(f64, usize)> = Vec::with_capacity(size);
    let mut idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; rv.dims.len()];
    for i in 0..size {
        let flat = rv.flat_offset(&idx);
        indexed.push((data[rv.base_off as usize + flat], i));
        crate::vm::increment_indices(&mut idx, &rv.dims);
    }
    if direction == 1 {
        indexed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        indexed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    let mut temp = vec![0.0f64; temp_slots];
    for (rank_0based, &(_, orig_idx)) in indexed.iter().enumerate() {
        temp[orig_idx] = (rank_0based + 1) as f64;
    }
    temp
}

fn assert_sort_order_matches(input: &StaticArrayView, direction: f64, data: &[f64], slots: usize) {
    let got = run_sort_op(
        input.clone(),
        Opcode::VectorSortOrder { write_temp_id: 0 },
        direction,
        data,
        slots,
        slots,
    );
    let want = vm_sort_order_oracle(input, direction.round() as i32, data, slots);
    assert_eq!(got, want, "sort_order direction {direction}");
}

fn assert_rank_matches(input: &StaticArrayView, direction: f64, data: &[f64], slots: usize) {
    let got = run_sort_op(
        input.clone(),
        Opcode::Rank { write_temp_id: 0 },
        direction,
        data,
        slots,
        slots,
    );
    let want = vm_rank_oracle(input, direction.round() as i32, data, slots);
    assert_eq!(got, want, "rank direction {direction}");
}

#[test]
fn vector_sort_order_1d_ascending_matches_vm() {
    // input [30, 10, 20, 40]; ascending -> the sorted in-row source indices are
    // [1 (10), 2 (20), 0 (30), 3 (40)].
    let input = dense_view(0, &[4]);
    let data = [30.0, 10.0, 20.0, 40.0];
    assert_sort_order_matches(&input, 1.0, &data, 4);
    let got = run_sort_op(
        input,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        1.0,
        &data,
        4,
        4,
    );
    assert_eq!(got, vec![1.0, 2.0, 0.0, 3.0]);
}

#[test]
fn vector_sort_order_1d_descending_matches_vm() {
    // direction != 1 sorts descending: [30,10,20,40] -> indices of [40,30,20,10]
    // = [3, 0, 2, 1].
    let input = dense_view(0, &[4]);
    let data = [30.0, 10.0, 20.0, 40.0];
    assert_sort_order_matches(&input, 0.0, &data, 4);
    let got = run_sort_op(
        input,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        0.0,
        &data,
        4,
        4,
    );
    assert_eq!(got, vec![3.0, 0.0, 2.0, 1.0]);
}

#[test]
fn vector_sort_order_tie_stability_matches_vm() {
    // Equal values keep input order (stable). [5, 5, 1, 5]: ascending sorts the
    // single 1 (index 2) first, then the three 5s in input order [0, 1, 3].
    let input = dense_view(0, &[4]);
    let data = [5.0, 5.0, 1.0, 5.0];
    assert_sort_order_matches(&input, 1.0, &data, 4);
    let got = run_sort_op(
        input,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        1.0,
        &data,
        4,
        4,
    );
    assert_eq!(got, vec![2.0, 0.0, 1.0, 3.0]);
}

#[test]
fn vector_sort_order_multi_row_matches_vm() {
    // A 2x3 source: each ROW is sorted independently (the innermost dim is the
    // sorted axis), and result indices are 0-based WITHIN the row. Row 0
    // [30,10,20] asc -> [1,2,0]; row 1 [5,9,7] asc -> [0,2,1]. The output is
    // row-major, so temp = [1,2,0, 0,2,1].
    let input = dense_view(0, &[2, 3]);
    let data = [30.0, 10.0, 20.0, 5.0, 9.0, 7.0];
    assert_sort_order_matches(&input, 1.0, &data, 6);
    let got = run_sort_op(
        input,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        1.0,
        &data,
        6,
        6,
    );
    assert_eq!(got, vec![1.0, 2.0, 0.0, 0.0, 2.0, 1.0]);
}

#[test]
fn vector_sort_order_nan_element_is_stable_like_vm() {
    // A NaN element compares Equal to everything (the VM's
    // partial_cmp.unwrap_or(Equal) under a stable sort), so it neither displaces
    // a non-NaN nor reorders -- it stays in input order. Cross-checked
    // element-for-element vs the sibling VM function.
    let input = dense_view(0, &[4]);
    let data = [3.0, f64::NAN, 1.0, 2.0];
    assert_sort_order_matches(&input, 1.0, &data, 4);
    assert_sort_order_matches(&input, 0.0, &data, 4);
}

#[test]
fn vector_sort_order_transposed_view_matches_vm() {
    // A non-contiguous (transposed) view exercises the strided element reads in
    // the gather. Cross-checked vs the sibling over every element.
    let view = StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[3, 2]),
        strides: SmallVec::from_slice(&[1, 3]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0, 0]),
    };
    assert!(!view.to_runtime_view().is_contiguous());
    let data = [11.0, 12.0, 13.0, 21.0, 22.0, 23.0];
    assert_sort_order_matches(&view, 1.0, &data, 6);
    assert_sort_order_matches(&view, 0.0, &data, 6);
}

#[test]
fn rank_whole_view_ascending_matches_vm() {
    // Rank over the WHOLE view, 1-based, indexed by ORIGINAL position. [30,10,20,
    // 40] ascending: 10 is rank 1, 20 rank 2, 30 rank 3, 40 rank 4, so the result
    // at the original positions is [3, 1, 2, 4].
    let input = dense_view(0, &[4]);
    let data = [30.0, 10.0, 20.0, 40.0];
    assert_rank_matches(&input, 1.0, &data, 4);
    let got = run_sort_op(input, Opcode::Rank { write_temp_id: 0 }, 1.0, &data, 4, 4);
    assert_eq!(got, vec![3.0, 1.0, 2.0, 4.0]);
}

#[test]
fn rank_whole_view_descending_matches_vm() {
    // Descending: 40 rank 1, 30 rank 2, 20 rank 3, 10 rank 4 -> [2, 4, 3, 1].
    let input = dense_view(0, &[4]);
    let data = [30.0, 10.0, 20.0, 40.0];
    assert_rank_matches(&input, 0.0, &data, 4);
    let got = run_sort_op(input, Opcode::Rank { write_temp_id: 0 }, 0.0, &data, 4, 4);
    assert_eq!(got, vec![2.0, 4.0, 3.0, 1.0]);
}

#[test]
fn rank_multi_dim_ranks_whole_view_not_per_row() {
    // Unlike VectorSortOrder, Rank ranks the WHOLE view (not per-row). A 2x3
    // view ranks all 6 cells together. Cross-checked vs the faithful oracle.
    let input = dense_view(0, &[2, 3]);
    let data = [30.0, 10.0, 20.0, 5.0, 9.0, 7.0];
    assert_rank_matches(&input, 1.0, &data, 6);
    // Sorted ascending: 5(idx3),9(idx4),7(idx5)... actually [5,7,9,10,20,30]
    // -> ranks at original positions: 30->6, 10->4, 20->5, 5->1, 9->3, 7->2.
    let got = run_sort_op(input, Opcode::Rank { write_temp_id: 0 }, 1.0, &data, 6, 6);
    assert_eq!(got, vec![6.0, 4.0, 5.0, 1.0, 3.0, 2.0]);
}

#[test]
fn rank_tie_stability_matches_vm() {
    // Equal values keep input order: [5, 5, 1, 5] ascending. The 1 (idx 2) is
    // rank 1; the three 5s get ranks 2, 3, 4 in input order (idx 0, 1, 3).
    let input = dense_view(0, &[4]);
    let data = [5.0, 5.0, 1.0, 5.0];
    assert_rank_matches(&input, 1.0, &data, 4);
    let got = run_sort_op(input, Opcode::Rank { write_temp_id: 0 }, 1.0, &data, 4, 4);
    assert_eq!(got, vec![2.0, 3.0, 1.0, 4.0]);
}

#[test]
fn rank_nan_element_matches_vm() {
    // A NaN element compares Equal (stable). Cross-checked vs the faithful oracle
    // (the NaN keeps its input position in the stable sort, so its rank is its
    // sorted slot among the Equal-treated elements).
    let input = dense_view(0, &[4]);
    let data = [3.0, f64::NAN, 1.0, 2.0];
    assert_rank_matches(&input, 1.0, &data, 4);
    assert_rank_matches(&input, 0.0, &data, 4);
}

/// Build `mat[rows][cols]` via `PushVarViewDirect`, dynamically subscript dim 0
/// with an out-of-range `row_1based` (so the resulting `cols`-element row view's
/// validity flag is 0), run `op` writing temp 0, and read back the `cols` temp
/// slots. An invalid input view must fill the whole temp region with NaN.
fn run_dyn_sort_op(rows: u16, cols: u16, row_1based: f64, op: Opcode, data: &[f64]) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.add_dim_list(2, [rows, cols, 0, 0]);
    context.set_temp_info(vec![0], cols as usize);
    let code = vec![
        Opcode::PushVarViewDirect {
            base_off: 0,
            dim_list_id: 0,
        },
        Opcode::LoadConstant { id: 0 }, // direction
        Opcode::LoadConstant { id: 1 }, // runtime row index (1-based)
        Opcode::ViewSubscriptDynamic { dim_idx: 0 },
        op,
        Opcode::PopView {},
    ];
    run_and_read_temps(
        &context,
        code,
        vec![1.0, row_1based],
        &seed_run(0, data),
        cols as usize,
    )
}

#[test]
fn vector_sort_order_invalid_view_fills_temp_with_nan() {
    // A 3x4 matrix; row 5 is out of range, so the dynamically-subscripted row
    // view is invalid and VectorSortOrder must fill the whole temp with NaN
    // (the VM's `!is_valid -> fill_temp_nan`).
    let data: Vec<f64> = (0..12).map(|i| i as f64).collect();
    let got = run_dyn_sort_op(
        3,
        4,
        5.0,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        &data,
    );
    assert!(
        got.iter().all(|v| v.is_nan()),
        "invalid view must fill the temp with NaN, got {got:?}"
    );
    // A valid row (row 2) writes real 0-based in-row ranks (no NaN).
    let ok = run_dyn_sort_op(
        3,
        4,
        2.0,
        Opcode::VectorSortOrder { write_temp_id: 0 },
        &data,
    );
    assert!(ok.iter().all(|v| !v.is_nan()), "valid row must not be NaN");
}

#[test]
fn rank_invalid_view_fills_temp_with_nan() {
    let data: Vec<f64> = (0..12).map(|i| i as f64).collect();
    let got = run_dyn_sort_op(3, 4, 5.0, Opcode::Rank { write_temp_id: 0 }, &data);
    assert!(
        got.iter().all(|v| v.is_nan()),
        "invalid view must fill the temp with NaN, got {got:?}"
    );
    let ok = run_dyn_sort_op(3, 4, 2.0, Opcode::Rank { write_temp_id: 0 }, &data);
    assert!(ok.iter().all(|v| !v.is_nan()), "valid row must not be NaN");
}

// ── LookupArray parity vs the VM (per-element arrayed GF) ─────────────────

// GF region base for the LookupArray tests: past the curr/next chunks
// (4096..8192), TEMP_BASE (8192), and VECTOR_SCRATCH_BASE (16384), within the
// harness's single 64 KiB page.
const LA_GF_BASE: u32 = 24576;

/// Seed `tables` into the GF directory + data regions at `LA_GF_BASE` (the
/// directory's N 8-byte entries, then each table's knots), matching the
/// production layout the `LookupArray`/`Lookup` opcodes read.
fn seed_gf_tables(tables: &[&[(f64, f64)]]) -> Vec<(u64, f64)> {
    let n = tables.len() as u32;
    let data_base = LA_GF_BASE + n * 8; // past the N directory entries
    let mut seed = Vec::new();
    let mut data_rel = 0u32;
    for (t, knots) in tables.iter().enumerate() {
        let abs = data_base + data_rel;
        seed.push((
            u64::from(LA_GF_BASE) + (t as u64) * 8,
            dir_entry_f64(abs, knots.len() as u32),
        ));
        for (k, &(x, y)) in knots.iter().enumerate() {
            let knot = u64::from(abs) + (k as u64) * 16;
            seed.push((knot, x));
            seed.push((knot + 8, y));
        }
        data_rel += knots.len() as u32 * 16;
    }
    seed
}

/// Run `PushStaticView(input); LookupArray{base_gf, table_count, mode}; PopView`
/// over the seeded GF tables, writing temp 0, and read back `temp_count` slots.
/// `index` (the shared scalar lookup index) is pushed beneath the opcode.
#[allow(clippy::too_many_arguments)]
fn run_lookup_array(
    input: StaticArrayView,
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    index: f64,
    tables: &[&[(f64, f64)]],
    temp_count: usize,
    temp_slots: usize,
    input_data: &[f64],
) -> Vec<f64> {
    let mut context = ByteCodeContext::default();
    context.set_temp_info(vec![0], temp_slots);
    let in_id = context.add_static_view(input);
    let ctx = EmitCtx {
        gf_directory_base: LA_GF_BASE,
        gf_data_base: LA_GF_BASE,
        temp_storage_base: TEMP_BASE,
        ctx: &context,
        ..ctx_with_cond_depth(0)
    };
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // index
        Opcode::PushStaticView { view_id: in_id },
        Opcode::LookupArray {
            base_gf,
            table_count,
            mode,
            write_temp_id: 0,
        },
        Opcode::PopView {},
    ];
    let mut seed = seed_run(0, input_data);
    seed.extend(seed_gf_tables(tables));
    let bytes = build_module(&bc(vec![index], code), &ctx, false, 0);
    let info = validate(&bytes).expect("emitted module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let mem = store
        .instance_export(inst, "mem")
        .unwrap()
        .as_mem()
        .unwrap();
    store.mem_access_mut_slice(mem, |b| {
        for &(addr, v) in &seed {
            let a = addr as usize;
            b[a..a + 8].copy_from_slice(&v.to_le_bytes());
        }
    });
    let eval = store
        .instance_export(inst, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(i32,), ()>(eval, (0_i32,))
        .expect("invoke");
    store.mem_access_mut_slice(mem, |b| {
        (0..temp_count)
            .map(|i| {
                let a = TEMP_BASE as usize + i * 8;
                f64::from_le_bytes(b[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Faithful oracle for `LookupArray` (mirroring `vm.rs:2586-2629`): for each
/// element `i`, `elem_off = flat_offset(indices)`; NaN if `elem_off >=
/// table_count`, else the VM lookup over `tables[base_gf + elem_off]` at `index`.
fn vm_lookup_array_oracle(
    input: &StaticArrayView,
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    index: f64,
    tables: &[&[(f64, f64)]],
    temp_slots: usize,
) -> Vec<f64> {
    let rv = input.to_runtime_view();
    let size = rv.size();
    let mut idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; rv.dims.len()];
    let mut temp = vec![0.0f64; temp_slots];
    for slot in temp.iter_mut().take(size) {
        let elem_off = rv.flat_offset(&idx);
        *slot = if elem_off >= table_count as usize {
            f64::NAN
        } else {
            let gf = tables[base_gf as usize + elem_off];
            vm_lookup_oracle(mode, gf, index)
        };
        crate::vm::increment_indices(&mut idx, &rv.dims);
    }
    temp
}

#[allow(clippy::too_many_arguments)]
fn assert_lookup_array_matches(
    input: &StaticArrayView,
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    index: f64,
    tables: &[&[(f64, f64)]],
    slots: usize,
    input_data: &[f64],
) {
    let got = run_lookup_array(
        input.clone(),
        base_gf,
        table_count,
        mode,
        index,
        tables,
        slots,
        slots,
        input_data,
    );
    let want = vm_lookup_array_oracle(input, base_gf, table_count, mode, index, tables, slots);
    assert_eq!(got.len(), want.len());
    for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
        if w.is_nan() {
            assert!(g.is_nan(), "lookup_array slot {i}: expected NaN, got {g}");
        } else {
            assert_eq!(g, w, "lookup_array slot {i}: got {g}, want {w}");
        }
    }
}

#[test]
fn lookup_array_interp_matches_vm() {
    // Three per-element tables; a contiguous 3-element input view -> elem_off
    // [0, 1, 2]. Each element looks up its own table at the shared index.
    let t0: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 100.0)]; // y = 10x
    let t1: &[(f64, f64)] = &[(0.0, 1.0), (10.0, 2.0)]; // y = x/10 + 1
    let t2: &[(f64, f64)] = &[(0.0, 5.0), (10.0, 5.0)]; // constant 5
    let tables = [t0, t1, t2];
    let input = dense_view(0, &[3]);
    let input_data = [0.0, 0.0, 0.0];
    assert_lookup_array_matches(
        &input,
        0,
        3,
        LookupMode::Interpolate,
        5.0,
        &tables,
        3,
        &input_data,
    );
    let got = run_lookup_array(
        input,
        0,
        3,
        LookupMode::Interpolate,
        5.0,
        &tables,
        3,
        3,
        &input_data,
    );
    // index 5: t0 interp 50, t1 interp 1.5, t2 constant 5.
    assert_eq!(got, vec![50.0, 1.5, 5.0]);
}

/// A monotonic-x table fixture (reused across modes/indices).
const LA_TABLE_A: &[(f64, f64)] = &[(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];
const LA_TABLE_B: &[(f64, f64)] = &[(0.0, 0.0), (2.0, 8.0), (2.0, 12.0), (5.0, 50.0)];

#[test]
fn lookup_array_all_modes_over_domain_match_vm() {
    // Two per-element tables, a 2-element input view (elem_off [0, 1]). For each
    // mode, probe several indices spanning below/at/between/above the knots; each
    // element's result must match the corresponding VM lookup over its table.
    let tables = [LA_TABLE_A, LA_TABLE_B];
    let input = dense_view(0, &[2]);
    let input_data = [0.0, 0.0];
    for mode in [
        LookupMode::Interpolate,
        LookupMode::Forward,
        LookupMode::Backward,
    ] {
        for &index in &[-1.0, 0.0, 0.5, 1.0, 2.0, 2.001, 3.25, 4.0, 100.0] {
            assert_lookup_array_matches(&input, 0, 2, mode, index, &tables, 2, &input_data);
        }
    }
}

#[test]
fn lookup_array_out_of_range_element_offset_is_nan() {
    // table_count = 2, but the input view has 3 elements -> elem_off [0, 1, 2].
    // Element 2's offset (2) is >= table_count (2), so its result is NaN
    // (matching the scalar Lookup bound), while elements 0 and 1 look up tables
    // 0 and 1.
    let tables = [LA_TABLE_A, LA_TABLE_B];
    let input = dense_view(0, &[3]);
    let input_data = [0.0, 0.0, 0.0];
    assert_lookup_array_matches(
        &input,
        0,
        2,
        LookupMode::Interpolate,
        1.0,
        &tables,
        3,
        &input_data,
    );
    let got = run_lookup_array(
        input,
        0,
        2,
        LookupMode::Interpolate,
        1.0,
        &tables,
        3,
        3,
        &input_data,
    );
    assert_eq!(got[0], 20.0); // t0 at index 1 (exact knot)
    assert!(got[2].is_nan(), "element offset 2 >= table_count 2 -> NaN");
}

#[test]
fn lookup_array_base_gf_offsets_into_directory() {
    // base_gf selects a starting table; a 2-element view with base_gf=1 reads
    // tables 1 and 2 (NOT 0 and 1). Three tables, table_count covers all three.
    let t0: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 100.0)];
    let t1: &[(f64, f64)] = &[(0.0, 1.0), (10.0, 2.0)];
    let t2: &[(f64, f64)] = &[(0.0, 7.0), (10.0, 7.0)];
    let tables = [t0, t1, t2];
    let input = dense_view(0, &[2]);
    let input_data = [0.0, 0.0];
    // base_gf=1, table_count=3 (the bound is on elem_off, not base_gf+elem_off,
    // matching the VM): elem_off [0,1], tables base_gf+elem_off = [1, 2].
    assert_lookup_array_matches(
        &input,
        1,
        3,
        LookupMode::Interpolate,
        5.0,
        &tables,
        2,
        &input_data,
    );
    let got = run_lookup_array(
        input,
        1,
        3,
        LookupMode::Interpolate,
        5.0,
        &tables,
        2,
        2,
        &input_data,
    );
    // t1 interp at 5 -> 1.5; t2 constant 7.
    assert_eq!(got, vec![1.5, 7.0]);
}

#[test]
fn lookup_array_strided_view_offsets_match_vm() {
    // A transposed (non-contiguous) input view exercises the per-element
    // flat_offset projection for elem_off. dim_ids/strides differ from row-major,
    // so a mis-addressed elem_off would pick the wrong table. Cross-checked vs the
    // faithful oracle, which uses the same `flat_offset`.
    let t0: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 100.0)];
    let t1: &[(f64, f64)] = &[(0.0, 1.0), (10.0, 2.0)];
    let t2: &[(f64, f64)] = &[(0.0, 20.0), (10.0, 30.0)];
    let t3: &[(f64, f64)] = &[(0.0, 5.0), (10.0, 5.0)];
    let tables = [t0, t1, t2, t3];
    // 2x2 transposed: dims [2,2], strides [1,2] -> elem_offs visited row-major
    // are [0, 2, 1, 3].
    let input = StaticArrayView {
        base_off: 0,
        is_temp: false,
        dims: SmallVec::from_slice(&[2, 2]),
        strides: SmallVec::from_slice(&[1, 2]),
        offset: 0,
        sparse: SmallVec::new(),
        dim_ids: SmallVec::from_slice(&[0, 0]),
    };
    let input_data = [0.0, 0.0, 0.0, 0.0];
    assert_lookup_array_matches(
        &input,
        0,
        4,
        LookupMode::Interpolate,
        5.0,
        &tables,
        4,
        &input_data,
    );
}

#[test]
fn lookup_array_invalid_view_fills_temp_with_nan() {
    // A dynamically-subscripted-out-of-range input view -> the whole temp region
    // is filled with NaN (the VM's `!is_valid -> fill_temp_nan`).
    let t0: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 100.0)];
    let tables = [t0, t0, t0, t0];
    let mut context = ByteCodeContext::default();
    context.add_dim_list(2, [3, 4, 0, 0]); // mat[3][4]
    context.set_temp_info(vec![0], 4);
    let ctx = EmitCtx {
        gf_directory_base: LA_GF_BASE,
        gf_data_base: LA_GF_BASE,
        temp_storage_base: TEMP_BASE,
        ctx: &context,
        ..ctx_with_cond_depth(0)
    };
    // mat[5, *]: row 5 out of range -> invalid 4-element row view.
    let code = vec![
        Opcode::LoadConstant { id: 0 }, // index
        Opcode::PushVarViewDirect {
            base_off: 0,
            dim_list_id: 0,
        },
        Opcode::LoadConstant { id: 1 }, // runtime row index (1-based)
        Opcode::ViewSubscriptDynamic { dim_idx: 0 },
        Opcode::LookupArray {
            base_gf: 0,
            table_count: 4,
            mode: LookupMode::Interpolate,
            write_temp_id: 0,
        },
        Opcode::PopView {},
    ];
    let mut seed = seed_run(0, &(0..12).map(|i| i as f64).collect::<Vec<_>>());
    seed.extend(seed_gf_tables(&tables));
    let bytes = build_module(&bc(vec![5.0, 5.0], code), &ctx, false, 0);
    let info = validate(&bytes).expect("module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate")
        .module_addr;
    let mem = store
        .instance_export(inst, "mem")
        .unwrap()
        .as_mem()
        .unwrap();
    store.mem_access_mut_slice(mem, |b| {
        for &(addr, v) in &seed {
            let a = addr as usize;
            b[a..a + 8].copy_from_slice(&v.to_le_bytes());
        }
    });
    let eval = store
        .instance_export(inst, "eval")
        .unwrap()
        .as_func()
        .unwrap();
    store
        .invoke_simple_typed::<(i32,), ()>(eval, (0_i32,))
        .expect("invoke");
    let temps: Vec<f64> = store.mem_access_mut_slice(mem, |b| {
        (0..4)
            .map(|i| {
                let a = TEMP_BASE as usize + i * 8;
                f64::from_le_bytes(b[a..a + 8].try_into().unwrap())
            })
            .collect()
    });
    assert!(
        temps.iter().all(|v| v.is_nan()),
        "invalid input view must fill the LookupArray temp with NaN, got {temps:?}"
    );
}
