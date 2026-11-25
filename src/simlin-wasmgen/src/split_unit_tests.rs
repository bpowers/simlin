// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Encoded partition mechanics. NOP spans deliberately isolate byte accounting
//! and function signatures; production boundary discovery and runtime semantics
//! are covered by `module::split_tests` using compiler-produced programs.

use super::*;
use wasm_encoder::ValType;

fn splitter(target_bytes: usize, max_bytes: usize, first_index: u32) -> ProgramSplitter {
    assert!(target_bytes <= max_bytes);
    ProgramSplitter {
        first_index,
        target_bytes,
        max_bytes,
        functions: Vec::new(),
    }
}

/// Each span is independently stack neutral and has no locals in flight. This
/// supplies legal boundaries by construction for testing the partitioner itself.
fn nop_spans(frame: &Function, spans: &[usize]) -> (Function, Vec<usize>) {
    let mut body = frame.clone();
    let mut offsets = Vec::new();
    for &len in spans {
        for _ in 0..len {
            body.instruction(&Instruction::Nop);
        }
        offsets.push(body.byte_len());
    }
    body.instruction(&Instruction::End);
    (body, offsets)
}

#[test]
fn duplicate_offsets_do_not_create_empty_helpers() {
    let frame = Function::new([]);
    let (body, offsets) = nop_spans(&frame, &[2, 2, 2]);
    let mut duplicate = vec![frame.byte_len(), frame.byte_len()];
    for &offset in &offsets {
        duplicate.extend([offset, offset]);
    }
    let mut once = splitter(4, 80, 17);
    let mut repeated = splitter(4, 80, 17);
    let expected = once
        .finish(frame.clone(), body.clone(), &offsets, 0)
        .unwrap();
    let actual = repeated.finish(frame, body, &duplicate, 0).unwrap();
    assert_eq!(expected.into_raw_body(), actual.into_raw_body());
    assert_eq!(once.functions.len(), 3);
    assert_eq!(repeated.functions.len(), 3);
    for (left, right) in once.functions.into_iter().zip(repeated.functions) {
        assert_eq!(left.body.into_raw_body(), right.body.into_raw_body());
    }
}

#[test]
fn byte_cap_includes_local_declarations_and_final_end() {
    // The 128-local group also covers a multibyte LEB local-count prefix.
    let frame = Function::new([(128, ValType::I32), (5, ValType::F64)]);
    let (body, offsets) = nop_spans(&frame, &[3, 3]);
    let exact_cap = frame.byte_len() + 3 + 1;
    let mut parts = splitter(exact_cap, exact_cap, 1);
    parts.finish(frame.clone(), body, &offsets, 0).unwrap();
    assert_eq!(parts.functions.len(), 2);
    for helper in parts.functions {
        assert_eq!(helper.body.byte_len(), exact_cap);
        assert!(check_size(&helper.body, exact_cap).is_ok());
        assert!(check_size(&helper.body, exact_cap - 1).is_err());
    }
    let (one_expression, offsets) = nop_spans(&frame, &[3]);
    let error = splitter(exact_cap - 1, exact_cap - 1, 1)
        .finish(frame, one_expression, &offsets, 0)
        .expect_err("one byte over the cap must be refused");
    assert!(error.to_string().contains("indivisible expression"));
}

#[test]
fn empty_programs_and_indivisible_expressions_do_not_add_dispatchers() {
    let frame = Function::new([(1, ValType::F64)]);
    let (empty, _) = nop_spans(&frame, &[]);
    let empty_bytes = empty.clone().into_raw_body();
    let mut parts = splitter(1, 80, 0);
    let actual = parts.finish(frame.clone(), empty, &[], 0).unwrap();
    assert_eq!(actual.into_raw_body(), empty_bytes);
    assert!(parts.functions.is_empty());

    let (expression, offsets) = nop_spans(&frame, &[12]);
    let expected = expression.clone().into_raw_body();
    let actual = parts.finish(frame, expression, &offsets, 0).unwrap();
    assert_eq!(actual.into_raw_body(), expected);
    assert!(parts.functions.is_empty());
}

#[test]
fn indivisible_span_above_hard_cap_is_rejected() {
    let frame = Function::new([]);
    let (body, offsets) = nop_spans(&frame, &[2, 19, 2]);
    let error = splitter(5, 20, 0)
        .finish(frame, body, &offsets, 0)
        .expect_err("the 21-byte helper cannot fit the hard cap");
    assert!(error.to_string().contains("indivisible expression"));
}

#[test]
fn dispatcher_cannot_exceed_the_hard_cap() {
    let frame = Function::new([]);
    let (body, offsets) = nop_spans(&frame, &[2, 2, 2, 2]);
    let mut parts = splitter(4, 20, 0);
    let error = parts
        .finish(frame, body, &offsets, 5)
        .expect_err("forwarding six parameters four times exceeds 20 bytes");
    assert!(error.to_string().contains("generated function"));
    assert!(
        parts
            .functions
            .iter()
            .all(|helper| helper.body.byte_len() <= 20)
    );
}

#[test]
fn dispatchers_forward_all_parameters_and_preserve_helper_local_prefixes() {
    let frame = Function::new([(1, ValType::I32), (2, ValType::F64)]);
    let (body, offsets) = nop_spans(&frame, &[2, 2]);
    let mut parts = splitter(frame.byte_len() + 3, 80, 127);
    let actual = parts.finish(frame.clone(), body, &offsets, 2).unwrap();
    let mut expected = Function::new([]);
    for index in [127, 128] {
        for parameter in 0..=2 {
            expected.instruction(&Instruction::LocalGet(parameter));
        }
        expected.instruction(&Instruction::Call(index));
    }
    expected.instruction(&Instruction::End);
    assert_eq!(actual.into_raw_body(), expected.into_raw_body());
    assert_eq!(parts.functions.len(), 2);
    let expected_helper = nop_spans(&frame, &[2]).0.into_raw_body();
    for helper in parts.functions {
        assert_eq!(helper.n_inputs, 2);
        assert_eq!(helper.body.into_raw_body(), expected_helper);
    }
}

#[test]
fn program_ending_with_live_evaluation_state_is_rejected() {
    let frame = Function::new([]);
    let mut body = frame.clone();
    body.instruction(&Instruction::F64Const(1.0.into()));
    body.instruction(&Instruction::End);
    let error = splitter(1, 80, 0)
        .finish(frame, body, &[], 0)
        .expect_err("a void phase cannot transfer a live arithmetic value to a helper");
    assert!(error.to_string().contains("live evaluation state"));
}
