// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the symbolic bytecode builder and its peephole optimizer -- the
//! literal pool, the emit-time next-assign fusion, and the pairwise
//! superinstruction fusion with its jump-offset fixup.
//!
//! These moved here wholesale when codegen started emitting symbolic opcodes
//! (GH #964): the builder and the peephole are address-independent, so they now
//! run before resolution, and there is no concrete-opcode twin of either left
//! to test.

use super::*;

/// The reference for slot `n` of the old concrete tests, as a distinct
/// variable name. The peephole never inspects a reference beyond copying it,
/// so all these assertions need is that distinct operands stay distinguishable
/// and survive fusion intact.
fn v(n: u16) -> SymVarRef {
    SymVarRef::base(crate::common::Ident::new(&format!("v{n}")))
}

#[test]
fn test_memoizing_interning() {
    let mut bytecode = SymbolicByteCodeBuilder::default();
    let a1 = bytecode.intern_literal(1.0);
    let b1 = bytecode.intern_literal(1.01);
    let b2 = bytecode.intern_literal(1.01);
    let b3 = bytecode.intern_literal(1.01);
    let a2 = bytecode.intern_literal(1.0);
    let b4 = bytecode.intern_literal(1.01);

    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    assert_eq!(b1, b3);
    assert_eq!(b1, b4);
    assert_ne!(a1, b1);

    let bytecode = bytecode.finish();
    assert_eq!(2, bytecode.literals.len());
}

#[test]
fn test_push_named_literal_no_dedup() {
    let mut builder = SymbolicByteCodeBuilder::default();
    let a = builder.push_named_literal(0.1);
    let b = builder.push_named_literal(0.1);
    let c = builder.push_named_literal(0.1);

    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
}

#[test]
#[should_panic(expected = "jump at pc 0 targets")]
fn test_peephole_panics_on_out_of_bounds_jump_target() {
    // A jump that targets beyond the code length indicates a compiler bug
    let mut bc = SymbolicByteCode {
        literals: vec![],
        code: vec![SymbolicOpcode::NextIterOrJump { jump_back: 10 }],
    };
    bc.peephole_optimize();
}

#[test]
fn test_peephole_empty_bytecode() {
    let mut bc = SymbolicByteCode {
        code: vec![],
        literals: vec![],
    };
    bc.peephole_optimize();
    assert!(bc.code.is_empty());
}

#[test]
fn test_peephole_single_instruction() {
    let mut bc = SymbolicByteCode {
        code: vec![SymbolicOpcode::Ret],
        literals: vec![],
    };
    bc.peephole_optimize();
    assert_eq!(bc.code.len(), 1);
    assert!(matches!(bc.code[0], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_no_fusible_patterns() {
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadVar { var: v(0) },
            SymbolicOpcode::LoadVar { var: v(1) },
            SymbolicOpcode::Not {},
            SymbolicOpcode::Ret,
        ],
        literals: vec![],
    };
    bc.peephole_optimize();
    assert_eq!(bc.code.len(), 4);
    assert_eq!(bc.code[0], SymbolicOpcode::LoadVar { var: v(0) });
    assert_eq!(bc.code[1], SymbolicOpcode::LoadVar { var: v(1) });
    assert!(matches!(bc.code[2], SymbolicOpcode::Not {}));
    assert!(matches!(bc.code[3], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_load_constant_assign_curr_fusion() {
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::AssignCurr { var: v(5) },
        ],
        literals: vec![42.0],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 1);
    match &bc.code[0] {
        SymbolicOpcode::AssignConstCurr { var, literal_id } => {
            assert_eq!(*var, v(5));
            assert_eq!(*literal_id, 0);
        }
        _ => panic!("expected AssignConstCurr"),
    }
}

#[test]
fn test_peephole_op2_assign_curr_fusion() {
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadVar { var: v(0) },
            SymbolicOpcode::LoadVar { var: v(1) },
            SymbolicOpcode::Op2 { op: Op2::Add },
            SymbolicOpcode::AssignCurr { var: v(2) },
        ],
        literals: vec![],
    };
    bc.peephole_optimize();

    // LoadVar, LoadVar stay; Op2+AssignCurr fuse into BinOpAssignCurr
    assert_eq!(bc.code.len(), 3);
    assert_eq!(bc.code[0], SymbolicOpcode::LoadVar { var: v(0) });
    assert_eq!(bc.code[1], SymbolicOpcode::LoadVar { var: v(1) });
    match &bc.code[2] {
        SymbolicOpcode::BinOpAssignCurr { op, var } => {
            assert!(matches!(op, Op2::Add));
            assert_eq!(*var, v(2));
        }
        _ => panic!("expected BinOpAssignCurr"),
    }
}

/// The next-value assign is fused at EMIT time (there is no un-fused
/// `AssignNext` opcode for the peephole to fuse), so this pins the
/// builder helper codegen uses.
#[test]
fn test_builder_fuses_trailing_op2_into_assign_next() {
    let mut builder = SymbolicByteCodeBuilder::default();
    builder.push_opcode(SymbolicOpcode::LoadVar { var: v(0) });
    builder.push_opcode(SymbolicOpcode::LoadVar { var: v(1) });
    builder.push_opcode(SymbolicOpcode::Op2 { op: Op2::Mul });
    assert!(builder.fuse_trailing_op2_into_assign_next(&v(3)));
    builder.push_opcode(SymbolicOpcode::Ret);

    let bc = builder.finish();
    assert_eq!(bc.code.len(), 4);
    match &bc.code[2] {
        SymbolicOpcode::BinOpAssignNext { op, var } => {
            assert!(matches!(op, Op2::Mul));
            assert_eq!(*var, v(3));
        }
        _ => panic!("expected BinOpAssignNext"),
    }
}

/// The other half of the contract: when the operand walk did NOT end in
/// an `Op2` -- the shape an eventual `non_negative` implementation
/// (GH #545) would produce by wrapping the stock update in a builtin --
/// the helper refuses and emits nothing, so codegen can raise a typed
/// error instead of silently dropping the store.
#[test]
fn test_builder_refuses_assign_next_fusion_without_trailing_op2() {
    let mut builder = SymbolicByteCodeBuilder::default();
    builder.push_opcode(SymbolicOpcode::LoadVar { var: v(0) });
    builder.push_opcode(SymbolicOpcode::Apply {
        func: BuiltinId::Max,
    });
    assert!(!builder.fuse_trailing_op2_into_assign_next(&v(3)));
    assert_eq!(builder.len(), 2, "a refused fusion must emit nothing");

    // ...and an empty stream is refused too, rather than panicking.
    let mut empty = SymbolicByteCodeBuilder::default();
    assert!(!empty.fuse_trailing_op2_into_assign_next(&v(0)));
    assert_eq!(empty.len(), 0);
}

#[test]
fn test_peephole_all_op2_variants_fuse() {
    // Verify every Op2 variant can be fused with AssignCurr
    let ops = [
        Op2::Add,
        Op2::Sub,
        Op2::Mul,
        Op2::Div,
        Op2::Exp,
        Op2::Mod,
        Op2::Gt,
        Op2::Gte,
        Op2::Lt,
        Op2::Lte,
        Op2::Eq,
        Op2::And,
        Op2::Or,
    ];
    for op in ops {
        let mut bc = SymbolicByteCode {
            code: vec![
                SymbolicOpcode::Op2 { op },
                SymbolicOpcode::AssignCurr { var: v(10) },
            ],
            literals: vec![],
        };
        bc.peephole_optimize();
        assert_eq!(bc.code.len(), 1, "failed for op variant");
        assert!(matches!(bc.code[0], SymbolicOpcode::BinOpAssignCurr { .. }));
    }
}

#[test]
fn test_peephole_multiple_fusions() {
    // Two independent fusion opportunities in sequence
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::AssignCurr { var: v(0) },
            SymbolicOpcode::LoadVar { var: v(1) },
            SymbolicOpcode::LoadVar { var: v(2) },
            SymbolicOpcode::Op2 { op: Op2::Sub },
            SymbolicOpcode::AssignCurr { var: v(3) },
        ],
        literals: vec![1.0],
    };
    bc.peephole_optimize();

    // LoadConstant+AssignCurr -> AssignConstCurr
    // LoadVar, LoadVar stay
    // Op2+AssignCurr -> BinOpAssignCurr
    assert_eq!(bc.code.len(), 4);
    assert!(matches!(bc.code[0], SymbolicOpcode::AssignConstCurr { .. }));
    assert_eq!(bc.code[1], SymbolicOpcode::LoadVar { var: v(1) });
    assert_eq!(bc.code[2], SymbolicOpcode::LoadVar { var: v(2) });
    assert!(matches!(bc.code[3], SymbolicOpcode::BinOpAssignCurr { .. }));
}

#[test]
fn test_peephole_mixed_fusible_and_nonfusible() {
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadVar { var: v(0) },
            SymbolicOpcode::Not {},
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::AssignCurr { var: v(1) },
            SymbolicOpcode::LoadVar { var: v(2) },
            SymbolicOpcode::Ret,
        ],
        literals: vec![0.0],
    };
    bc.peephole_optimize();

    // LoadVar, Not stay; LoadConstant+AssignCurr fuse; LoadVar, Ret stay
    assert_eq!(bc.code.len(), 5);
    assert_eq!(bc.code[0], SymbolicOpcode::LoadVar { var: v(0) });
    assert!(matches!(bc.code[1], SymbolicOpcode::Not {}));
    assert!(matches!(bc.code[2], SymbolicOpcode::AssignConstCurr { .. }));
    assert_eq!(bc.code[3], SymbolicOpcode::LoadVar { var: v(2) });
    assert!(matches!(bc.code[4], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_jump_target_prevents_fusion() {
    // If instruction i+1 is a jump target, don't fuse i with i+1.
    // Layout (before optimization):
    //   0: LoadConstant { id: 0 }       <- loop body start (jump target)
    //   1: AssignCurr { off: 0 }
    //   2: NextIterOrJump { jump_back: -2 }  (target = 2 + (-2) = 0)
    //   3: Ret
    //
    // Instruction 0 is a jump target, so even though 0 is LoadConstant
    // and 1 is AssignCurr, we should NOT fuse them because instruction 0
    // is a jump target. Wait -- actually the check is whether i+1 is a
    // jump target. Here instruction 0 IS a jump target. The optimizer checks
    // `!jump_targets[i + 1]` to decide whether to fuse i with i+1.
    //
    // For i=0: jump_targets[1] is false, so fusion IS allowed.
    // The jump target protection matters when the SECOND instruction of a
    // potential pair is a jump target. Let's build that scenario:
    //
    //   0: Ret                            <- something before the loop
    //   1: LoadVar { off: 5 }             <- jump target (loop body start)
    //   2: NextIterOrJump { jump_back: -1 }  (target = 2 + (-1) = 1)
    //   3: Ret
    //
    // For i=0 (Ret): can_fuse checks jump_targets[1] = true -> no fusion.
    // This prevents fusing Ret with LoadVar, which is correct.
    //
    // A more realistic scenario: Op2 followed by AssignCurr where the
    // AssignCurr is a jump target.
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::Op2 { op: Op2::Add },             // 0
            SymbolicOpcode::AssignCurr { var: v(0) },         // 1 -- jump target
            SymbolicOpcode::NextIterOrJump { jump_back: -1 }, // 2 -> target = 2-1 = 1
            SymbolicOpcode::Ret,                              // 3
        ],
        literals: vec![],
    };
    bc.peephole_optimize();

    // Fusion of 0+1 should be prevented because instruction 1 is a jump target
    assert_eq!(bc.code.len(), 4);
    assert!(matches!(bc.code[0], SymbolicOpcode::Op2 { op: Op2::Add }));
    assert_eq!(bc.code[1], SymbolicOpcode::AssignCurr { var: v(0) });
    assert!(matches!(bc.code[2], SymbolicOpcode::NextIterOrJump { .. }));
    assert!(matches!(bc.code[3], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_jump_target_only_blocks_specific_pair() {
    // Verify that a jump target only blocks fusion of the pair where
    // the second instruction is the target, not other pairs.
    //
    //   0: LoadConstant { id: 0 }
    //   1: AssignCurr { off: 0 }         <- NOT a jump target, so 0+1 CAN fuse
    //   2: LoadVar { off: 5 }            <- jump target
    //   3: NextIterOrJump { jump_back: -1 }  (target = 3-1 = 2)
    //   4: Ret
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::AssignCurr { var: v(0) },
            SymbolicOpcode::LoadVar { var: v(5) },
            SymbolicOpcode::NextIterOrJump { jump_back: -1 },
            SymbolicOpcode::Ret,
        ],
        literals: vec![1.0],
    };
    bc.peephole_optimize();

    // 0+1 should fuse (neither target), 2 stays (it's a jump target, but
    // the previous instruction was AssignCurr which doesn't match any pattern
    // anyway), 3 stays, 4 stays
    assert_eq!(bc.code.len(), 4);
    assert_eq!(
        bc.code[0],
        SymbolicOpcode::AssignConstCurr {
            var: v(0),
            literal_id: 0
        }
    );
    assert_eq!(bc.code[1], SymbolicOpcode::LoadVar { var: v(5) });
    assert!(matches!(bc.code[2], SymbolicOpcode::NextIterOrJump { .. }));
    assert!(matches!(bc.code[3], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_jump_offset_recalculation_next_iter() {
    // When fusion shrinks the code, jump offsets must be recalculated.
    // This test places a fusion BEFORE the loop (outside the jump target
    // to jump instruction range) so the fixup works correctly.
    //
    // Before optimization:
    //   0: LoadConstant { id: 0 }    \
    //   1: AssignCurr { off: 0 }     / -> fuse
    //   2: LoadVar { off: 1 }        <- jump target
    //   3: AssignCurr { off: 2 }
    //   4: NextIterOrJump { jump_back: -2 }  target = 4+(-2) = 2
    //   5: Ret
    //
    // After optimization:
    //   0: AssignConstCurr            (fused 0+1)
    //   1: LoadVar { off: 1 }         (jump target)
    //   2: AssignCurr { off: 2 }
    //   3: NextIterOrJump { jump_back: -2 }  (loop body unchanged)
    //   4: Ret
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },           // 0
            SymbolicOpcode::AssignCurr { var: v(0) },         // 1
            SymbolicOpcode::LoadVar { var: v(1) },            // 2 (jump target)
            SymbolicOpcode::AssignCurr { var: v(2) },         // 3
            SymbolicOpcode::NextIterOrJump { jump_back: -2 }, // 4, target=2
            SymbolicOpcode::Ret,                              // 5
        ],
        literals: vec![1.0],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 5);
    assert!(matches!(bc.code[0], SymbolicOpcode::AssignConstCurr { .. }));
    assert_eq!(bc.code[1], SymbolicOpcode::LoadVar { var: v(1) });
    assert_eq!(bc.code[2], SymbolicOpcode::AssignCurr { var: v(2) });
    match &bc.code[3] {
        SymbolicOpcode::NextIterOrJump { jump_back } => {
            assert_eq!(*jump_back, -2, "jump_back should remain -2");
        }
        _ => panic!("expected NextIterOrJump"),
    }
    assert!(matches!(bc.code[4], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_fusion_inside_loop_body() {
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadVar { var: v(0) },    // 0 (jump target)
            SymbolicOpcode::Op2 { op: Op2::Add },     // 1 \
            SymbolicOpcode::AssignCurr { var: v(1) }, // 2 / fuse
            SymbolicOpcode::NextIterOrJump { jump_back: -3 }, // 3, target=0
            SymbolicOpcode::Ret,                      // 4
        ],
        literals: vec![],
    };
    bc.peephole_optimize();

    // 1+2 fuse -> BinOpAssignCurr
    // Result: [LoadVar, BinOpAssignCurr, NextIterOrJump, Ret]
    assert_eq!(bc.code.len(), 4);
    assert_eq!(bc.code[0], SymbolicOpcode::LoadVar { var: v(0) });
    assert_eq!(
        bc.code[1],
        SymbolicOpcode::BinOpAssignCurr {
            op: Op2::Add,
            var: v(1)
        }
    );
    match &bc.code[2] {
        SymbolicOpcode::NextIterOrJump { jump_back } => {
            // new PC 2, target should be new PC 0 -> jump_back = -2
            assert_eq!(*jump_back, -2);
        }
        other => panic!(
            "expected NextIterOrJump, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert!(matches!(bc.code[3], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_no_fusion_when_patterns_dont_match() {
    // Op2 followed by something other than AssignCurr
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::Op2 { op: Op2::Add },
            SymbolicOpcode::Not {},
            SymbolicOpcode::Ret,
        ],
        literals: vec![],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 3);
    assert!(matches!(bc.code[0], SymbolicOpcode::Op2 { op: Op2::Add }));
    assert!(matches!(bc.code[1], SymbolicOpcode::Not {}));
}

#[test]
fn test_peephole_load_constant_not_followed_by_assign_curr() {
    // LoadConstant not followed by AssignCurr should not fuse
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::Not {},
            SymbolicOpcode::Ret,
        ],
        literals: vec![1.0],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 3);
    assert!(matches!(bc.code[0], SymbolicOpcode::LoadConstant { id: 0 }));
}

#[test]
fn test_peephole_via_builder() {
    // Verify that SymbolicByteCodeBuilder::finish() runs peephole_optimize
    let mut builder = SymbolicByteCodeBuilder::default();
    let lit_id = builder.intern_literal(3.125);
    builder.push_opcode(SymbolicOpcode::LoadConstant { id: lit_id });
    builder.push_opcode(SymbolicOpcode::AssignCurr { var: v(7) });
    builder.push_opcode(SymbolicOpcode::Ret);

    let bc = builder.finish();
    assert_eq!(bc.code.len(), 2);
    match &bc.code[0] {
        SymbolicOpcode::AssignConstCurr { var, literal_id } => {
            assert_eq!(*var, v(7));
            assert_eq!(*literal_id, lit_id);
        }
        _ => panic!("expected AssignConstCurr after builder finish"),
    }
    assert!(matches!(bc.code[1], SymbolicOpcode::Ret));
}

#[test]
fn test_peephole_consecutive_fusions_chain() {
    // Three consecutive fusible pairs
    let mut bc = SymbolicByteCode {
        code: vec![
            SymbolicOpcode::LoadConstant { id: 0 },
            SymbolicOpcode::AssignCurr { var: v(0) },
            SymbolicOpcode::LoadConstant { id: 1 },
            SymbolicOpcode::AssignCurr { var: v(1) },
            SymbolicOpcode::Op2 { op: Op2::Div },
            SymbolicOpcode::AssignCurr { var: v(2) },
        ],
        literals: vec![1.0, 2.0],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 3);
    assert_eq!(
        bc.code[0],
        SymbolicOpcode::AssignConstCurr {
            var: v(0),
            literal_id: 0
        }
    );
    assert_eq!(
        bc.code[1],
        SymbolicOpcode::AssignConstCurr {
            var: v(1),
            literal_id: 1
        }
    );
    match &bc.code[2] {
        SymbolicOpcode::BinOpAssignCurr { op, var } => {
            assert!(matches!(op, Op2::Div));
            assert_eq!(*var, v(2));
        }
        _ => panic!("expected BinOpAssignCurr"),
    }
}

#[test]
fn test_peephole_last_instruction_not_fused_alone() {
    // If the fusible first instruction is the very last one, no fusion happens
    let mut bc = SymbolicByteCode {
        code: vec![SymbolicOpcode::Ret, SymbolicOpcode::LoadConstant { id: 0 }],
        literals: vec![1.0],
    };
    bc.peephole_optimize();

    assert_eq!(bc.code.len(), 2);
    assert!(matches!(bc.code[0], SymbolicOpcode::Ret));
    assert!(matches!(bc.code[1], SymbolicOpcode::LoadConstant { id: 0 }));
}
