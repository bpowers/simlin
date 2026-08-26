// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Pins for how a fragment numbers its temps (`ast::TempAllocator`).
//!
//! A fragment is one variable's one phase, the unit `compiler::Var::new`
//! lowers, and its temp ids are the contract assembly builds on:
//! `FragmentMerger`'s `Recycle` strategy max-merges fragment temp `t` onto
//! shared slot `t`, so the ids must be 0-based and dense, and codegen sizes
//! the temp region from `temp_sizes`, so every id must carry a size. The
//! rows below are derived from the enumeration of the `TempAllocator::alloc`
//! call sites and of the paths that reach each one, and every site has a row:
//!
//! * Pass 1 (`ast/expr3.rs`): a reducer's computed operand
//!   (`scalar_pass1_decompositions_take_distinct_ids`, and the recycling
//!   rows), the same operand inside the hoisting paths, where no element
//!   scope is open (`plain_arm_beside_a_hoisting_arm_keeps_a_fresh_id_per_element`,
//!   `a2a_top_level_hoist_over_a_pass1_operand`,
//!   `a2a_per_element_top_level_hoist_over_a_pass1_operand`,
//!   `a2a_per_element_nested_hoist_beside_a_pass1_temp`), and a per-element
//!   arrayed-GF apply
//!   (`pass1_arrayed_gf_apply_takes_an_id_from_the_same_allocator`,
//!   `pass1_arrayed_gf_apply_recycles_across_elements`);
//! * `replace_nested_builtins_for_element`, reached from the scalar hoist
//!   (`scalar_nested_array_builtins_take_distinct_ids`), the nested
//!   apply-to-all branches (`a2a_shared_nested_hoists_take_distinct_ids`,
//!   `a2a_per_element_nested_hoists_take_one_id_per_element`), and every
//!   arrayed arm kind: the hoisting arm shared
//!   (`arrayed_arms_each_hoisting_take_distinct_ids`,
//!   `arrayed_hoisting_arm_after_plain_arms_starts_at_zero`,
//!   `arrayed_default_and_override_hoists_take_distinct_ids`), an explicit
//!   arm hoisted in place (the same rows, and
//!   `explicit_arm_beside_a_hoisting_arm_reads_its_own_pass1_temp`), the
//!   EXCEPT default shared when it is not the hoisting arm
//!   (`default_arm_beside_a_hoisting_override_reads_its_own_pass1_temp`,
//!   `default_arm_with_two_pass1_temps_reads_both_of_its_own`,
//!   `two_d_override_beside_a_hoisting_default_reads_its_own_pass1_temp`),
//!   and `ArmHoist::PerElement` for the hoisting arm
//!   (`dim_dependent_hoisting_default_hoists_per_element`) and for a default
//!   that is not the hoisting arm
//!   (`dim_dependent_default_beside_a_hoisting_override_hoists_per_element`);
//! * the top-level apply-to-all hoists
//!   (`a2a_shared_top_level_hoist_takes_one_id`,
//!   `a2a_per_element_top_level_hoist_takes_one_id_per_element`, and the
//!   Pass 1 operand rows above);
//! * the computed-operand materializer
//!   (`materialized_operands_after_recycled_elements_take_fresh_ids`).
//!
//! Each row checks the same three properties of the production-lowered
//! `Vec<Expr>`:
//!
//! 1. **dense**: the ids written by `AssignTemp` are exactly `0..n`;
//! 2. **well-formed**: every `TempArray` / `TempArrayElement` read is
//!    preceded, in evaluation order, by a write of the id it names;
//! 3. **sized**: `temp_sizes` of the emitted fragment lists exactly those ids,
//!    each with the largest view written to it.
//!
//! Rows additionally pin the *shape* of the sequence. A hoist writes each
//! temp exactly once, so the hoisting rows assert distinct ids. A plain
//! apply-to-all or arrayed equation evaluates its elements in sequence and
//! each element's Pass 1 temp is dead before the next element runs, so those
//! rows assert the elements share ONE id: an equation over 300 elements costs
//! one temp slot, not 300, which is what keeps it inside the bytecode's `u8`
//! `TempId`. One row pins the seam between the two -- a materialized operand
//! spliced into recycled element code takes an id no element wrote. The
//! arrayed-arm rows also assert simulated values: an arm's Pass 1 temp is
//! read by that arm's own expression, never by another arm's hoist (the
//! design plan's "Phase 2a semantic divergences"), and the expected numbers
//! are derived in each docstring from the builtins' rules rather than copied
//! from a run.
//!
//! Every fixture is lowered through `lower_var_fragment`, exactly as
//! `compile_var_fragment` lowers it, and emitted through
//! `compile_var_fragment` itself. The last test is the one deliberate
//! exception: it hand-builds a non-dense expression list that no lowering
//! produces, to pin the emitter's loud refusal of one.

use std::collections::BTreeSet;

use super::var_fragment::{LoweredVarFragment, lower_var_fragment};
use super::*;
use crate::compiler::{Expr, SubscriptIndex};
use crate::test_common::TestProject;

/// The shared fixture: three-element `d`, two-element `e`, one matrix over
/// both, and a per-element sort direction so a builtin's scalar argument can
/// vary with the active element.
fn fixture(name: &str) -> TestProject {
    TestProject::new(name)
        .indexed_dimension("d", 3)
        .indexed_dimension("e", 2)
        .array_with_ranges("vals[d]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_with_ranges("bump[d]", vec![("1", "0"), ("2", "100"), ("3", "0")])
        .array_with_ranges("dir[d]", vec![("1", "1"), ("2", "-1"), ("3", "1")])
        .array_with_ranges(
            "matrix[e,d]",
            vec![
                ("1,1", "1"),
                ("1,2", "2"),
                ("1,3", "3"),
                ("2,1", "10"),
                ("2,2", "20"),
                ("2,3", "30"),
            ],
        )
}

/// One temp event of a fragment, in evaluation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempEvent {
    /// `AssignTemp(id, ..)` writing a view of `size` elements.
    Write { id: u32, size: usize },
    /// `TempArray(id, ..)` or `TempArrayElement(id, ..)`.
    Read { id: u32 },
}

/// The temp events of `exprs`, in the order the VM evaluates them: an
/// assignment's right-hand side before the assignment, operands left to
/// right.
fn temp_events(exprs: &[Expr]) -> Vec<TempEvent> {
    fn walk(expr: &Expr, out: &mut Vec<TempEvent>) {
        match expr {
            Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::StaticSubscript(_, _, _)
            | Expr::Dt(_)
            | Expr::ModuleInput(_, _) => {}
            Expr::TempArray(id, _, _) | Expr::TempArrayElement(id, _, _, _) => {
                out.push(TempEvent::Read { id: *id });
            }
            Expr::Subscript(_, indices, _, _) => {
                for idx in indices {
                    match idx {
                        SubscriptIndex::Single(e) => walk(e, out),
                        SubscriptIndex::Range(lo, hi) => {
                            walk(lo, out);
                            walk(hi, out);
                        }
                    }
                }
            }
            Expr::App(builtin, _) => {
                for arg in builtin.args() {
                    walk(arg, out);
                }
            }
            Expr::EvalModule(_, _, _, args) => {
                for arg in args {
                    walk(arg, out);
                }
            }
            Expr::Op2(_, lhs, rhs, _) => {
                walk(lhs, out);
                walk(rhs, out);
            }
            Expr::Op1(_, inner, _) => walk(inner, out),
            Expr::If(cond, t, f, _) => {
                walk(cond, out);
                walk(t, out);
                walk(f, out);
            }
            Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) => walk(rhs, out),
            Expr::AssignTemp(id, rhs, view) => {
                walk(rhs, out);
                out.push(TempEvent::Write {
                    id: *id,
                    size: view.dims.iter().product(),
                });
            }
        }
    }
    let mut out = Vec::new();
    for expr in exprs {
        walk(expr, &mut out);
    }
    out
}

/// The production lowering and emission of `var`'s non-initial phase: the
/// lowered `Vec<Expr>` from `lower_var_fragment` (called as
/// `compile_var_fragment` calls it) and the `temp_sizes` of the flow fragment
/// `compile_var_fragment` emits from it.
fn flow_fragment(project: &TestProject, var: &str) -> (Vec<Expr>, Vec<(u32, usize)>) {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &datamodel).project;
    let model = *source_project
        .models(&db)
        .get("main")
        .expect("fixture has a main model");
    let source_var = *model
        .variables(&db)
        .get(var)
        .unwrap_or_else(|| panic!("fixture declares `{var}`"));

    let dim_context = project_dimensions_context(&db, source_project);
    let converted_dims = project_converted_dimensions(&db, source_project);
    let model_name_ident = Ident::new(model.name(&db));
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let module_models = model_module_map(&db, model, source_project).clone();
    let lowered = lower_var_fragment(
        &db,
        source_var,
        model,
        source_project,
        &[],
        converted_dims,
        dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );
    let exprs = match lowered {
        LoweredVarFragment::Lowered {
            per_phase_lowered, ..
        } => {
            per_phase_lowered
                .noninitial
                .unwrap_or_else(|e| panic!("`{var}` must lower: {e:?}"))
                .ast
        }
        LoweredVarFragment::Fatal { fatal_diags, .. } => {
            panic!("`{var}` must lower, got {fatal_diags:?}")
        }
    };

    let input_set = ModuleInputSet::from_names(&db, &[]);
    let fragment = compile_var_fragment(&db, source_var, model, source_project, input_set)
        .as_ref()
        .unwrap_or_else(|| panic!("`{var}` must emit a fragment"));
    let temp_sizes = fragment
        .fragment
        .flow_bytecodes
        .as_ref()
        .unwrap_or_else(|| panic!("`{var}` must emit a flow fragment"))
        .temp_sizes
        .clone();
    (exprs, temp_sizes)
}

/// The three properties every row asserts. Returns the events so a row can
/// go on to assert the shape of the sequence.
fn assert_dense_well_formed_and_sized(
    events: &[TempEvent],
    temp_sizes: &[(u32, usize)],
    what: &str,
) {
    let mut written: BTreeSet<u32> = BTreeSet::new();
    let mut max_size: std::collections::BTreeMap<u32, usize> = Default::default();
    for event in events {
        match *event {
            TempEvent::Write { id, size } => {
                written.insert(id);
                let entry = max_size.entry(id).or_insert(0);
                *entry = (*entry).max(size);
            }
            TempEvent::Read { id } => {
                assert!(
                    written.contains(&id),
                    "{what}: temp {id} is read before any write of it; events {events:?}"
                );
            }
        }
    }
    let n = written.len() as u32;
    assert_eq!(
        written,
        (0..n).collect::<BTreeSet<u32>>(),
        "{what}: written temp ids must be exactly 0..{n}; events {events:?}"
    );
    let expected_sizes: Vec<(u32, usize)> = max_size.into_iter().collect();
    assert_eq!(
        temp_sizes,
        &expected_sizes[..],
        "{what}: the emitted fragment's temp_sizes must list every written id \
         with the largest view written to it"
    );
}

/// The ids written, in write order (with repeats).
fn writes(events: &[TempEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            TempEvent::Write { id, .. } => Some(*id),
            TempEvent::Read { .. } => None,
        })
        .collect()
}

fn assert_each_written_once(events: &[TempEvent], expected_n: usize, what: &str) {
    let writes = writes(events);
    let distinct: BTreeSet<u32> = writes.iter().copied().collect();
    assert_eq!(
        writes.len(),
        distinct.len(),
        "{what}: a hoisted temp is written exactly once; writes {writes:?}"
    );
    assert_eq!(
        distinct.len(),
        expected_n,
        "{what}: expected {expected_n} distinct temps, got {distinct:?}"
    );
}

// ---------------------------------------------------------------------------
// Scalar equations: Pass 1 decomposition and the nested-builtin hoist.
// ---------------------------------------------------------------------------

/// Pass 1 decomposes two reducer operands in one scalar equation into two
/// distinct temps.
#[test]
fn scalar_pass1_decompositions_take_distinct_ids() {
    let project = fixture("p1_scalar").aux("s", "SUM(vals[*] * 2) + SUM(bump[*] + 1)", None);
    let (exprs, temp_sizes) = flow_fragment(&project, "s");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "scalar pass 1");
    assert_each_written_once(&events, 2, "scalar pass 1");
}

/// Two array-producing builtins nested under reducers in one scalar equation
/// are hoisted into two distinct temps.
#[test]
fn scalar_nested_array_builtins_take_distinct_ids() {
    let project = fixture("hoist_scalar").aux(
        "s",
        "SUM(VECTOR SORT ORDER(vals[*], 1)) + SUM(RANK(bump[*], 1))",
        None,
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "s");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "scalar nested hoist");
    assert_each_written_once(&events, 2, "scalar nested hoist");
}

// ---------------------------------------------------------------------------
// Apply-to-all equations: the hoisting branches.
// ---------------------------------------------------------------------------

/// A top-level array-producing builtin whose arguments do not vary with the
/// element is hoisted once and read per element.
#[test]
fn a2a_shared_top_level_hoist_takes_one_id() {
    let project = fixture("a2a_top_shared").array_aux("out[d]", "VECTOR SORT ORDER(vals[d], 1)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a shared top-level");
    assert_each_written_once(&events, 1, "a2a shared top-level");
}

/// A top-level array-producing builtin whose scalar argument varies with the
/// element is re-evaluated per element, one temp each.
#[test]
fn a2a_per_element_top_level_hoist_takes_one_id_per_element() {
    let project =
        fixture("a2a_top_per_elem").array_aux("out[d]", "VECTOR SORT ORDER(vals[d], dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element top-level");
    assert_each_written_once(&events, 3, "a2a per-element top-level");
}

/// Two array-producing builtins nested in one element-invariant expression
/// are hoisted once each and read per element.
#[test]
fn a2a_shared_nested_hoists_take_distinct_ids() {
    let project = fixture("a2a_nested_shared")
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[d], 1) + RANK(bump[d], 1)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a shared nested");
    assert_each_written_once(&events, 2, "a2a shared nested");
}

/// A nested array-producing builtin whose scalar argument varies with the
/// element is hoisted per element.
#[test]
fn a2a_per_element_nested_hoists_take_one_id_per_element() {
    let project = fixture("a2a_nested_per_elem")
        .array_aux("out[d]", "10 + VECTOR SORT ORDER(vals[d], dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element nested");
    assert_each_written_once(&events, 3, "a2a per-element nested");
}

// ---------------------------------------------------------------------------
// Arrayed equations: per-element arms.
// ---------------------------------------------------------------------------

/// Every arm hoists its own array-producing builtin: one temp per arm.
#[test]
fn arrayed_arms_each_hoisting_take_distinct_ids() {
    let project = fixture("arrayed_arms").array_with_ranges(
        "out[d]",
        vec![
            ("1", "SUM(RANK(vals[*], 1))"),
            ("2", "SUM(VECTOR SORT ORDER(bump[*], 1))"),
            ("3", "SUM(RANK(dir[*], -1))"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "arrayed arms");
    assert_each_written_once(&events, 3, "arrayed arms");
}

/// The hoisting arm is not the first element, so the classification of the
/// earlier, plain arms is discarded before the hoist is emitted; the ids the
/// fragment keeps still start at 0.
#[test]
fn arrayed_hoisting_arm_after_plain_arms_starts_at_zero() {
    let project = fixture("arrayed_later_arm").array_with_ranges(
        "out[d]",
        vec![
            ("1", "vals[1] * 2"),
            ("2", "SUM(RANK(vals[*], 1))"),
            ("3", "bump[3]"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "arrayed later arm");
    assert_each_written_once(&events, 1, "arrayed later arm");
}

/// An EXCEPT default that hoists, shared by the elements without an override,
/// beside an override arm that hoists on its own.
#[test]
fn arrayed_default_and_override_hoists_take_distinct_ids() {
    let project = fixture("arrayed_default_override").array_with_default_and_overrides(
        "out[d]",
        "SUM(RANK(vals[*], 1))",
        vec![("2", "SUM(VECTOR SORT ORDER(bump[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "arrayed default + override");
    assert_each_written_once(&events, 2, "arrayed default + override");
}

// ---------------------------------------------------------------------------
// Plain element loops: the elements share one id.
// ---------------------------------------------------------------------------

/// A plain apply-to-all equation whose body Pass 1 decomposes per element
/// writes ONE temp once per element, each write followed by the element that
/// reads it.
#[test]
fn a2a_elements_without_hoisting_recycle_one_id() {
    let project = fixture("a2a_recycle").array_aux("out[e]", "SUM(matrix[e, *] * 2)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a recycle");
    assert_eq!(
        events,
        vec![
            TempEvent::Write { id: 0, size: 3 },
            TempEvent::Read { id: 0 },
            TempEvent::Write { id: 0, size: 3 },
            TempEvent::Read { id: 0 },
        ],
        "two elements, one recycled temp, each read after its own write"
    );
}

/// Plain arrayed arms recycle the same way, and the shared slot is sized for
/// the largest arm: `matrix[*, 1]` is the two-element `e` column, the second
/// arm is three wide, and the one slot holds three.
#[test]
fn arrayed_arms_without_hoisting_recycle_one_id_sized_for_the_largest() {
    let project = fixture("arrayed_recycle").array_with_ranges(
        "out[e]",
        vec![
            ("1", "SUM(matrix[*, 1] * 2)"),
            ("2", "SUM(vals[*] + bump[*] + 1)"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "arrayed recycle");
    assert_eq!(
        events,
        vec![
            TempEvent::Write { id: 0, size: 2 },
            TempEvent::Read { id: 0 },
            TempEvent::Write { id: 0, size: 3 },
            TempEvent::Read { id: 0 },
        ],
        "both arms write temp 0, two wide then three wide"
    );
    assert_eq!(
        temp_sizes,
        vec![(0, 3)],
        "the slot is sized for the wider arm"
    );
}

/// The seam between the two regimes. After the element loop has recycled temp
/// 0, the operand materializer splices a temp of its own in front of each
/// element's reader; that temp must not alias the recycled one, which is
/// still live inside the element, so it takes an id no element wrote.
#[test]
fn materialized_operands_after_recycled_elements_take_fresh_ids() {
    let project = fixture("recycle_then_materialize")
        .array_aux("out[e]", "SUM(matrix[e, *] * 2) + SUM(ABS(matrix[e, *]))");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "recycle then materialize");
    let writes = writes(&events);
    assert_eq!(
        writes.iter().filter(|&&id| id == 0).count(),
        2,
        "the Pass 1 temp is recycled across the two elements; writes {writes:?}"
    );
    let fresh: BTreeSet<u32> = writes.iter().copied().filter(|&id| id != 0).collect();
    assert_eq!(
        fresh,
        BTreeSet::from([1, 2]),
        "each element's materialized operand takes its own id above the recycled \
         range; writes {writes:?}"
    );
    assert_eq!(temp_sizes, vec![(0, 3), (1, 3), (2, 3)]);
}

// ---------------------------------------------------------------------------
// Arrayed equations: an arm's Pass 1 temp beside another arm's hoist.
//
// Each of these rows also asserts the simulated values. The rules the
// expectations follow from: `SUM(vals[*] * 2)` over the fixture's
// `vals = [30, 10, 20]` is `2 * 60 = 120`; `SUM(bump[*] + 1)` over
// `bump = [0, 100, 0]` is `103`; `RANK` gives its `n` elements the distinct
// 1-based ranks `1..=n` (a stable sort by value, ties kept in position), so
// `SUM(RANK(..))` over three elements is `1 + 2 + 3 = 6` whatever the values;
// `VECTOR SORT ORDER` over three elements is a permutation of the 0-based
// positions `0..=2` in either direction, so its sum is `3`.
// ---------------------------------------------------------------------------

/// An EXCEPT default that is NOT the hoisting arm (element 1's override is)
/// and whose expression holds a Pass 1 temp beside its hoisted builtin. The
/// override's hoist takes id 0; the default, classified the first time an
/// element evaluates it, takes id 1 for its Pass 1 operand and id 2 for its
/// hoist, and element 3 reads the same two temps at its own index.
///
/// Values: the override is `6`; the default is `120 + 6 = 126`. A default
/// arm's Pass 1 temp reads its own operand, never the override's hoist (which
/// would make the default `6 + 6 = 12`).
#[test]
fn default_arm_beside_a_hoisting_override_reads_its_own_pass1_temp() {
    let project = fixture("default_not_hoisting").array_with_default_and_overrides(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(RANK(bump[*], 1))",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "default not hoisting arm");
    assert_each_written_once(&events, 3, "default not hoisting arm");
    project.assert_vm_result("out", &[6.0, 126.0, 126.0]);
}

/// The explicit-arm twin: element 1's arm is the hoisting arm, element 2's
/// own arm holds a Pass 1 temp beside its hoist, element 3 is plain.
/// Values: `6`, `120 + 6 = 126`, `vals[3] = 20`.
#[test]
fn explicit_arm_beside_a_hoisting_arm_reads_its_own_pass1_temp() {
    let project = fixture("explicit_not_hoisting").array_with_ranges(
        "out[d]",
        vec![
            ("1", "SUM(RANK(vals[*], 1))"),
            ("2", "SUM(vals[*] * 2) + SUM(RANK(bump[*], 1))"),
            ("3", "vals[3]"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "explicit not hoisting arm");
    assert_each_written_once(&events, 3, "explicit not hoisting arm");
    project.assert_vm_result("out", &[6.0, 126.0, 20.0]);
}

/// Two Pass 1 temps beside the hoist in a default that is not the hoisting
/// arm: ids 0 (override hoist), 1 and 2 (the default's two operands), 3 (the
/// default's hoist). Values: `6`, `120 + 103 + 6 = 229`.
#[test]
fn default_arm_with_two_pass1_temps_reads_both_of_its_own() {
    let project = fixture("default_two_pass1").array_with_default_and_overrides(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(bump[*] + 1) + SUM(RANK(bump[*], 1))",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "default with two pass 1 temps");
    assert_each_written_once(&events, 4, "default with two pass 1 temps");
    project.assert_vm_result("out", &[6.0, 229.0, 229.0]);
}

/// The 2-D spelling. The default is the hoisting arm (element `[1,1]`
/// evaluates it), and because `vals[d]` makes its lowered form vary with the
/// element it is `ArmHoist::PerElement`: each of the five default elements
/// hoists its own `RANK`. The override at `[2,1]` holds a Pass 1 temp beside
/// a hoist of its own. Ids, in element order: 0, 1, 2 (defaults `[1,*]`),
/// 3 and 4 (the override's operand and hoist), 5, 6 (defaults `[2,2]`,
/// `[2,3]`). Values, row-major: the default is `6 + vals[d]` = `36, 16, 26`;
/// the override is `120 + 6 = 126`.
#[test]
fn two_d_override_beside_a_hoisting_default_reads_its_own_pass1_temp() {
    let project = fixture("two_d_override").array_with_default_and_overrides(
        "out[e,d]",
        "SUM(RANK(vals[*], 1)) + vals[d]",
        vec![("2,1", "SUM(vals[*] * 2) + SUM(RANK(bump[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "2-D override");
    assert_each_written_once(&events, 7, "2-D override");
    project.assert_vm_result("out", &[36.0, 16.0, 26.0, 126.0, 16.0, 26.0]);
}

/// The hoisting arm's lowered form varies with the element (`dir[d]`), so it
/// is `ArmHoist::PerElement`: elements 1 and 3 each hoist their own sort.
/// Values: `3`, the plain override `bump[2] = 100`, `3`.
#[test]
fn dim_dependent_hoisting_default_hoists_per_element() {
    let project = fixture("dim_dependent_hoisting").array_with_default_and_overrides(
        "out[d]",
        "SUM(VECTOR SORT ORDER(vals[*], dir[d]))",
        vec![("2", "bump[2]")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "dim-dependent hoisting arm");
    assert_each_written_once(&events, 2, "dim-dependent hoisting arm");
    project.assert_vm_result("out", &[3.0, 100.0, 3.0]);
}

/// A dim-dependent default that is NOT the hoisting arm: classified once, at
/// element 2, as `ArmHoist::PerElement`, so elements 2 and 3 each hoist their
/// own sort beside their own Pass 1 operand. Ids: 0 (override hoist), 1 and 2
/// (element 2), 3 and 4 (element 3). Values: `6`, `3 + 120 = 123`, `123`.
#[test]
fn dim_dependent_default_beside_a_hoisting_override_hoists_per_element() {
    let project = fixture("dim_dependent_default").array_with_default_and_overrides(
        "out[d]",
        "SUM(VECTOR SORT ORDER(vals[*], dir[d])) + SUM(vals[*] * 2)",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "dim-dependent default");
    assert_each_written_once(&events, 5, "dim-dependent default");
    project.assert_vm_result("out", &[6.0, 123.0, 123.0]);
}

// ---------------------------------------------------------------------------
// Pass 1's other allocation: the per-element arrayed-GF apply.
// ---------------------------------------------------------------------------

/// A Vensim-syntax model with per-element lookup tables, the shape the MDL
/// importer produces for a table holder: `g[COP]` reads 100, 200, 300 at
/// `Time = 1` (0 at `Time = 0`), `g2[COP, ROW]` reads 10, 20, 30, 40
/// row-major, and `v[COP] = 1`.
fn gf_fixture(consumers: &str) -> TestProject {
    let mdl = format!(
        "{{UTF-8}}\n\
         COP: a, b, c ~~|\n\
         ROW: p, q ~~|\n\
         g[a]( (0,0),(1,100) ) ~~|\n\
         g[b]( (0,0),(1,200) ) ~~|\n\
         g[c]( (0,0),(1,300) ) ~~|\n\
         g2[a,p]( (0,0),(1,10) ) ~~|\n\
         g2[a,q]( (0,0),(1,20) ) ~~|\n\
         g2[b,p]( (0,0),(1,30) ) ~~|\n\
         g2[b,q]( (0,0),(1,40) ) ~~|\n\
         g2[c,p]( (0,0),(1,50) ) ~~|\n\
         g2[c,q]( (0,0),(1,60) ) ~~|\n\
         v[COP] = 1 ~~|\n\
         {consumers}\
         INITIAL TIME = 0 ~~|\n\
         FINAL TIME = 1 ~~|\n\
         SAVEPER = 1 ~~|\n\
         TIME STEP = 1 ~~|\n"
    );
    TestProject::from_datamodel(crate::open_vensim(&mdl).expect("the fixture MDL parses"))
}

/// `LOOKUP(g, Time)` over the three-element table holder is decomposed by
/// Pass 1 into a temp of its own (codegen's `LookupArray`), from the same
/// allocator as the reducer operand beside it: two distinct ids. Value series
/// for the scalar: `0 + 6` at `Time = 0`, `600 + 6` at `Time = 1`.
#[test]
fn pass1_arrayed_gf_apply_takes_an_id_from_the_same_allocator() {
    let project = gf_fixture("s = SUM(LOOKUP(g, Time)) + SUM(v[COP!] * 2) ~~|\n");
    let (exprs, temp_sizes) = flow_fragment(&project, "s");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "scalar gf apply");
    assert_each_written_once(&events, 2, "scalar gf apply");
    project.assert_vm_result("s", &[6.0, 606.0]);
}

/// In an apply-to-all equation the per-element GF apply is a Pass 1 temp like
/// any other, so the elements recycle it: `SUM(LOOKUP(g2, Time))` over the
/// 2-D holder sums each element's own row into one shared temp. Values at
/// `Time = 1`: `10 + 20`, `30 + 40`, `50 + 60`.
#[test]
fn pass1_arrayed_gf_apply_recycles_across_elements() {
    let project = gf_fixture("out[COP] = SUM(LOOKUP(g2, Time)) ~~|\n");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a gf apply");
    assert_eq!(
        writes(&events),
        vec![0, 0, 0],
        "three elements, one recycled temp"
    );
    assert_eq!(temp_sizes, vec![(0, 2)]);
    project.assert_vm_result("out", &[30.0, 70.0, 110.0]);
}

// ---------------------------------------------------------------------------
// Pass 1 temps inside the hoisting paths. No element scope is open there: a
// shared arm's Pass 1 pre-expression is emitted once and read by every later
// element that evaluates the arm, so it stays live across elements and a
// plain arm's temp cannot reuse its id. Every Pass 1 temp in these paths is
// therefore fresh, unlike in a plain equation.
// ---------------------------------------------------------------------------

/// A plain arm holding a Pass 1 temp beside a hoisting arm: element 1's hoist
/// takes id 0, and the plain arms of elements 2 and 3 take fresh ids 1 and 2
/// rather than sharing one as the arms of a plain equation do. Values: `6`,
/// `120`, `103`.
#[test]
fn plain_arm_beside_a_hoisting_arm_keeps_a_fresh_id_per_element() {
    let project = fixture("plain_beside_hoisting").array_with_ranges(
        "out[d]",
        vec![
            ("1", "SUM(RANK(vals[*], 1))"),
            ("2", "SUM(vals[*] * 2)"),
            ("3", "SUM(bump[*] + 1)"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "plain arm beside hoisting arm");
    assert_each_written_once(&events, 3, "plain arm beside hoisting arm");
    project.assert_vm_result("out", &[6.0, 120.0, 103.0]);
}

/// A top-level array-producing builtin whose operand is a Pass 1 temp:
/// `vals[*] * 2` takes id 0, the shared `RANK` hoist id 1. Values: the
/// ascending ranks of `[60, 20, 40]` are `3, 1, 2`.
#[test]
fn a2a_top_level_hoist_over_a_pass1_operand() {
    let project = fixture("a2a_top_pass1").array_aux("out[d]", "RANK(vals[*] * 2, 1)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a top-level over pass 1");
    assert_each_written_once(&events, 2, "a2a top-level over pass 1");
    project.assert_vm_result("out", &[3.0, 1.0, 2.0]);
}

/// The per-element twin: element 0's operand and hoist take ids 0 and 1,
/// and each further element re-lowers its operand (2, 4) and hoists its own
/// sort (3, 5). Values: the sort order of `[60, 20, 40]` ascending is
/// `[1, 2, 0]` and descending `[0, 2, 1]`; element 1 reads position 0 of the
/// ascending order, element 2 position 1 of the descending, element 3
/// position 2 of the ascending: `1, 2, 0`.
#[test]
fn a2a_per_element_top_level_hoist_over_a_pass1_operand() {
    let project = fixture("a2a_top_per_elem_pass1")
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[*] * 2, dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element over pass 1");
    assert_each_written_once(&events, 6, "a2a per-element over pass 1");
    project.assert_vm_result("out", &[1.0, 2.0, 0.0]);
}

/// A nested per-element hoist beside a Pass 1 temp. The classifying
/// lowering's pre-expression (id 0) is kept in front of the element code and
/// every element then re-lowers its own operand and hoists its own sort
/// (1 and 2, 3 and 4, 5 and 6), so id 0 is written and never read -- the
/// emission order the Phase 1 commit produces, kept for identity with it;
/// Phase 6(b) removes this path. Values: `120 + 3 = 123` for every element.
#[test]
fn a2a_per_element_nested_hoist_beside_a_pass1_temp() {
    let project = fixture("a2a_nested_per_elem_pass1").array_aux(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(VECTOR SORT ORDER(bump[*], dir[d]))",
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a nested per-element pass 1");
    assert_each_written_once(&events, 7, "a2a nested per-element pass 1");
    let read: BTreeSet<u32> = events
        .iter()
        .filter_map(|e| match e {
            TempEvent::Read { id } => Some(*id),
            TempEvent::Write { .. } => None,
        })
        .collect();
    assert_eq!(
        read,
        (1..7).collect::<BTreeSet<u32>>(),
        "temp 0 is the classifying lowering's operand, written and unread"
    );
    project.assert_vm_result("out", &[123.0, 123.0, 123.0]);
}

// ---------------------------------------------------------------------------
// The emitter's guard on the contract every row above establishes.
// ---------------------------------------------------------------------------

/// `compile_phase_to_per_var_bytecodes_reporting` sizes the temp table by the
/// number of distinct ids and indexes it by id, so it can only be right for
/// dense ids. No lowering produces a gap -- `Var::new` debug-asserts that the
/// ids written equal the ids the allocator issued -- so this input is
/// hand-built on purpose: it is the one shape production never supplies, and
/// the emitter must refuse it loudly rather than drop the temp's size.
#[test]
fn emitter_refuses_a_non_dense_temp_id() {
    use crate::ast::{ArrayView, Loc};
    use crate::compiler::VarRef;
    use crate::db::assemble::{compile_phase_to_per_var_bytecodes_reporting, fragment_emit_ctx};

    let model_name = Ident::new("main");
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let var_sizes = crate::compiler::VarSizes::new();
    let tables = std::collections::HashMap::new();
    let dimensions: Vec<crate::dimensions::Dimension> = Vec::new();
    let base = fragment_emit_ctx(&model_name, &inputs, &var_sizes, &tables, &dimensions);
    let view = ArrayView::contiguous(vec![2]);
    // Temp 1 is written and read; temp 0 is never written.
    let exprs = vec![
        Expr::AssignTemp(1, Box::new(Expr::Const(1.0, Loc::default())), view.clone()),
        Expr::AssignCurr(
            VarRef::base(Ident::new("x")),
            Box::new(Expr::App(
                crate::compiler::BuiltinFn::Sum(Box::new(Expr::TempArray(1, view, Loc::default()))),
                Loc::default(),
            )),
        ),
    ];
    let err = compile_phase_to_per_var_bytecodes_reporting(&base, &exprs)
        .expect_err("a non-dense temp id must be refused");
    assert!(
        err.contains("not dense"),
        "the refusal must say the ids are not dense, got: {err}"
    );
}
