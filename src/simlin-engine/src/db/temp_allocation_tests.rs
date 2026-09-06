// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Pins for how a fragment numbers its temps (`ast::TempAllocator`).
//!
//! A fragment is one variable's one phase, the unit `compiler::Var::new`
//! lowers, and its temp ids are the contract assembly builds on:
//! `FragmentMerger`'s `Recycle` strategy max-merges fragment temp `t` onto
//! shared slot `t`, so the ids must be 0-based and dense, and codegen sizes
//! the temp region from `temp_sizes`, so every id must carry a size.
//!
//! One pass issues them: `compiler::array_operand`, which materializes every
//! array value codegen cannot express in place. The rows below are derived from
//! its two enumerations.
//!
//! **Where it fires** -- the positions the signature table names:
//!
//! * an `ArgKind::Array` operand that is not already a view, in a scalar
//!   equation (`scalar_computed_operands_take_distinct_ids`), in an
//!   apply-to-all body (`a2a_elements_recycle_one_id`,
//!   `a2a_shared_operand_takes_one_id`) and in an arrayed arm
//!   (`arrayed_arms_recycle_one_id_sized_for_the_largest`,
//!   `arrayed_default_shares_its_operand_across_the_elements_that_evaluate_it`);
//! * a `ResultKind::Array` call in an ARRAY position, read whole
//!   (`scalar_nested_array_builtins_take_distinct_ids`);
//! * a `ResultKind::Array` call in a SCALAR position, read at the assignment's
//!   own element (`a2a_shared_top_level_hoist_takes_one_id`,
//!   `a2a_per_element_top_level_hoist_recycles_one_id`,
//!   `a2a_shared_nested_hoists_take_distinct_ids`,
//!   `a2a_per_element_nested_hoists_recycle_one_id`, and every arrayed-arm row);
//! * a per-element arrayed-GF apply, the one non-`ResultKind::Array` call that
//!   also writes a temp (`arrayed_gf_apply_takes_an_id_from_the_same_allocator`,
//!   `arrayed_gf_apply_recycles_across_elements`).
//!
//! **Which id it gets** -- the two regimes, decided by structural identity of
//! the lowered body:
//!
//! * SHARED, when two or more elements read the same body: one id for the whole
//!   equation, its `AssignTemp` hoisted ahead of the element code. Every row
//!   whose name says `shared`, plus `arrayed_default_shares_...`.
//! * RECYCLED, when one element reads it: an id the elements reissue
//!   (`TempAllocator::element_scopes`), so an equation over 300 elements costs
//!   one temp slot rather than 300 -- which is what keeps it inside the
//!   bytecode's `u8` `TempId`. Every row whose name says `recycle` or
//!   `per_element`.
//! * Both at once, which is the seam:
//!   `a2a_per_element_hoist_over_a_shared_operand` and
//!   `dim_dependent_default_beside_a_hoisting_override_recycles_per_element`
//!   hoist an element-invariant operand once and re-evaluate the builtin that
//!   reads it per element, and the shared id sits BELOW the recycled range so no
//!   element can clobber it.
//!
//! What the exact id sequences below pin: the two regimes and their relative
//! order -- shared ids below the recycled range, every element reissuing the
//! same recycled ids. That a row reads `[0, 1, 1, 1]` rather than
//! `[1, 0, 0, 0]` is the materializer's emission order, not the contract.
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
//! The arrayed-arm rows also assert simulated values: an arm's operand temp is
//! read by that arm's own expression, never by another arm's (the design plan's
//! "Phase 2a semantic divergences"), and the expected numbers are derived in
//! each docstring from the builtins' rules rather than copied from a run.
//!
//! Every fixture is lowered through the explicit `FragmentInput` constructor
//! and `lower_fragment`, exactly as `compile_var_fragment` lowers it, and
//! emitted through `compile_var_fragment` itself. The last test is the one
//! deliberate exception: it hand-builds a non-dense expression list that no
//! lowering produces, to pin the emitter's loud refusal of one.

use std::collections::BTreeSet;

use super::var_fragment::{ExplicitFragment, explicit_fragment_input};
use super::*;
use crate::compiler::fragment::lower_fragment;
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
/// lowered `Vec<Expr>` from the explicit `FragmentInput` constructor and
/// `lower_fragment` (called as `compile_var_fragment` calls them) and the
/// `temp_sizes` of the flow fragment `compile_var_fragment` emits from it.
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

    let ExplicitFragment { diagnostics, input } = explicit_fragment_input(
        &db,
        source_var,
        model,
        source_project,
        &[],
        crate::db::LtmOverlay::Off,
    );
    let input = input.unwrap_or_else(|| panic!("`{var}` must lower, got {diagnostics:?}"));
    let exprs = lower_fragment(&input, false)
        .unwrap_or_else(|e| panic!("`{var}` must lower: {e:?}"))
        .ast;

    let input_set = ModuleInputSet::from_names(&db, &[]);
    let fragment = compile_var_fragment(
        &db,
        source_var,
        model,
        source_project,
        input_set,
        crate::db::LtmOverlay::Off,
    )
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
// Scalar equations: one element, so every temp is that element's own.
// ---------------------------------------------------------------------------

/// Two computed reducer operands in one scalar equation take two distinct
/// temps: they are different bodies, so neither shares with the other, and one
/// element has nothing to recycle against.
#[test]
fn scalar_computed_operands_take_distinct_ids() {
    let project = fixture("scalar_operands").aux("s", "SUM(vals[*] * 2) + SUM(bump[*] + 1)", None);
    let (exprs, temp_sizes) = flow_fragment(&project, "s");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "scalar computed operands");
    assert_each_written_once(&events, 2, "scalar computed operands");
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
/// element is a different body per element, so it is re-evaluated per element
/// and the elements RECYCLE one id.
#[test]
fn a2a_per_element_top_level_hoist_recycles_one_id() {
    let project =
        fixture("a2a_top_per_elem").array_aux("out[d]", "VECTOR SORT ORDER(vals[d], dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element top-level");
    assert_eq!(
        writes(&events),
        vec![0, 0, 0],
        "three elements, one recycled temp"
    );
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

/// The same, nested inside arithmetic rather than at the top of the body.
#[test]
fn a2a_per_element_nested_hoists_recycle_one_id() {
    let project = fixture("a2a_nested_per_elem")
        .array_aux("out[d]", "10 + VECTOR SORT ORDER(vals[d], dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element nested");
    assert_eq!(
        writes(&events),
        vec![0, 0, 0],
        "three elements, one recycled temp"
    );
}

// ---------------------------------------------------------------------------
// Arrayed equations: per-element arms.
// ---------------------------------------------------------------------------

/// Every arm materializes its own array-producing builtin. Each body belongs to
/// one element, so the three arms RECYCLE one id rather than taking three.
#[test]
fn arrayed_arms_each_hoisting_recycle_one_id() {
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
    assert_eq!(
        writes(&events),
        vec![0, 0, 0],
        "three arms, one recycled temp"
    );
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

/// An EXCEPT default two elements evaluate, beside a one-element override: the
/// default's body is SHARED (one id, hoisted ahead of the elements) and the
/// override's is that element's own (a recycled id above it).
#[test]
fn arrayed_default_shares_its_operand_across_the_elements_that_evaluate_it() {
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

/// An apply-to-all equation whose operand varies with the element writes ONE
/// temp once per element, each write followed by the element that reads it.
#[test]
fn a2a_elements_recycle_one_id() {
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

/// Arrayed arms recycle the same way, and the one slot is sized for the largest
/// arm: `matrix[*, 1]` is the two-element `e` column, the second arm is three
/// wide, and the one slot holds three.
#[test]
fn arrayed_arms_recycle_one_id_sized_for_the_largest() {
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

/// Two computed operands in one element-varying body take two ids, and both
/// recycle: they are simultaneously live WITHIN an element and dead between
/// elements, which is exactly what the two axes of the numbering say.
#[test]
fn two_operands_in_one_varying_element_recycle_both_ids() {
    let project = fixture("recycle_two_operands")
        .array_aux("out[e]", "SUM(matrix[e, *] * 2) + SUM(ABS(matrix[e, *]))");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "two recycled operands");
    assert_eq!(
        writes(&events),
        vec![0, 1, 0, 1],
        "each element writes both temps, and the second element reuses the ids"
    );
    assert_eq!(temp_sizes, vec![(0, 3), (1, 3)]);
}

// ---------------------------------------------------------------------------
// Arrayed equations: one arm's operand temp beside another arm's.
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

/// An EXCEPT default two elements share, beside a one-element override, each
/// holding a computed operand beside an array-producing builtin. The default's
/// two bodies are SHARED (ids 0 and 1, hoisted ahead of the elements); the
/// override's is its own, on a recycled id above them.
///
/// Values: the override is `6`; the default is `120 + 6 = 126`. An arm's
/// operand temp reads its own operand, never another arm's (which would make
/// the default `6 + 6 = 12`) -- the design plan's "Phase 2a semantic
/// divergences".
#[test]
fn default_arm_beside_an_override_reads_its_own_operand_temp() {
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

/// The explicit-arm twin: three arms, each read by ONE element, so nothing is
/// shared and the two ids element 2 needs are recycled from element 1's one.
/// Values: `6`, `120 + 6 = 126`, `vals[3] = 20`.
#[test]
fn explicit_arm_beside_another_reads_its_own_operand_temp() {
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
    assert_eq!(
        writes(&events),
        vec![0, 0, 1],
        "no arm is shared, so element 2's two temps recycle element 1's one"
    );
    project.assert_vm_result("out", &[6.0, 126.0, 20.0]);
}

/// Three bodies in the shared default beside the override's one: the default's
/// take the shared ids 0, 1 and 2 and the override's the recycled id 3.
/// Values: `6`, `120 + 103 + 6 = 229`.
#[test]
fn default_arm_with_two_operand_temps_reads_both_of_its_own() {
    let project = fixture("default_two_operand_temps").array_with_default_and_overrides(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(bump[*] + 1) + SUM(RANK(bump[*], 1))",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "default with two operand temps");
    assert_each_written_once(&events, 4, "default with two operand temps");
    project.assert_vm_result("out", &[6.0, 229.0, 229.0]);
}

/// The 2-D spelling, and the row that shows the decision is made on the BODY
/// rather than on the arm. The default `SUM(RANK(vals[*], 1)) + vals[d]` varies
/// with the element -- but only in the `vals[d]` term, which materializes
/// nothing; its `RANK(vals[*], 1)` is the same body in all five default
/// elements, so it is SHARED on one id. The override at `[2,1]` adds its own
/// two, recycled above it: three temps in all.
///
/// Values, row-major: the default is `6 + vals[d]` = `36, 16, 26`; the override
/// is `120 + 6 = 126`.
#[test]
fn two_d_override_beside_a_shared_default_reads_its_own_operand_temp() {
    let project = fixture("two_d_override").array_with_default_and_overrides(
        "out[e,d]",
        "SUM(RANK(vals[*], 1)) + vals[d]",
        vec![("2,1", "SUM(vals[*] * 2) + SUM(RANK(bump[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "2-D override");
    assert_each_written_once(&events, 3, "2-D override");
    project.assert_vm_result("out", &[36.0, 16.0, 26.0, 126.0, 16.0, 26.0]);
}

/// The default's body varies with the element (`dir[d]`), so elements 1 and 3
/// each evaluate their own sort -- on one RECYCLED id, because neither reads
/// the other's.
/// Values: `3`, the plain override `bump[2] = 100`, `3`.
#[test]
fn dim_dependent_default_recycles_one_id_per_element() {
    let project = fixture("dim_dependent_hoisting").array_with_default_and_overrides(
        "out[d]",
        "SUM(VECTOR SORT ORDER(vals[*], dir[d]))",
        vec![("2", "bump[2]")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "dim-dependent hoisting arm");
    assert_eq!(
        writes(&events),
        vec![0, 0],
        "the two elements that evaluate the default recycle one id"
    );
    project.assert_vm_result("out", &[3.0, 100.0, 3.0]);
}

/// The seam: one arm, two regimes. The default's `SUM(vals[*] * 2)` is the same
/// body in elements 2 and 3, so it is SHARED on id 0; its
/// `VECTOR SORT ORDER(vals[*], dir[d])` differs, so each element evaluates its
/// own on the recycled id 1 -- which the override at element 1 also uses. The
/// shared id sits BELOW the recycled range, so no element clobbers it.
/// Values: `6`, `3 + 120 = 123`, `123`.
#[test]
fn dim_dependent_default_beside_a_hoisting_override_recycles_per_element() {
    let project = fixture("dim_dependent_default").array_with_default_and_overrides(
        "out[d]",
        "SUM(VECTOR SORT ORDER(vals[*], dir[d])) + SUM(vals[*] * 2)",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "dim-dependent default");
    assert_eq!(
        writes(&events),
        vec![0, 1, 1, 1],
        "the shared operand is written once ahead of the elements, and the three \
         per-element sorts recycle one id above it"
    );
    project.assert_vm_result("out", &[6.0, 123.0, 123.0]);
}

// ---------------------------------------------------------------------------
// The one call that writes a temp without a `ResultKind::Array` result: the
// per-element arrayed-GF apply.
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

/// `LOOKUP(g, Time)` over the three-element table holder takes a temp of its
/// own (codegen's `LookupArray`), from the same allocator as the reducer
/// operand beside it: two distinct ids. Value series for the scalar: `0 + 6` at
/// `Time = 0`, `600 + 6` at `Time = 1`.
#[test]
fn arrayed_gf_apply_takes_an_id_from_the_same_allocator() {
    let project = gf_fixture("s = SUM(LOOKUP(g, Time)) + SUM(v[COP!] * 2) ~~|\n");
    let (exprs, temp_sizes) = flow_fragment(&project, "s");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "scalar gf apply");
    assert_each_written_once(&events, 2, "scalar gf apply");
    project.assert_vm_result("s", &[6.0, 606.0]);
}

/// In an apply-to-all equation the per-element GF apply is a temp like any
/// other, and each element's is a different body (its own row of the table), so
/// the elements recycle one id: `SUM(LOOKUP(g2, Time))` over the 2-D holder
/// sums each element's own row. Values at `Time = 1`: `10 + 20`, `30 + 40`,
/// `50 + 60`.
#[test]
fn arrayed_gf_apply_recycles_across_elements() {
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
// A computed operand feeding an array-producing builtin: two temps, and the
// two regimes can differ between them.
// ---------------------------------------------------------------------------

/// Three arms, each with a body of its own and no arm shared, so all three
/// recycle one id -- a reducer arm beside an array-producing one is not a
/// different regime. Values: `6`, `120`, `103`.
#[test]
fn arms_that_share_nothing_recycle_one_id() {
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
    assert_eq!(
        writes(&events),
        vec![0, 0, 0],
        "three arms, one recycled temp"
    );
    project.assert_vm_result("out", &[6.0, 120.0, 103.0]);
}

/// A top-level array-producing builtin over a computed operand, both
/// element-invariant: `vals[*] * 2` takes shared id 0 and the `RANK` that reads
/// it shared id 1. Values: the ascending ranks of `[60, 20, 40]` are `3, 1, 2`.
#[test]
fn a2a_shared_hoist_over_a_shared_operand() {
    let project = fixture("a2a_top_shared").array_aux("out[d]", "RANK(vals[*] * 2, 1)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a top-level over an operand");
    assert_each_written_once(&events, 2, "a2a top-level over an operand");
    project.assert_vm_result("out", &[3.0, 1.0, 2.0]);
}

/// The mixed twin, and the reason the regime is decided per BODY rather than
/// per equation: the operand `vals[*] * 2` is the same in every element and is
/// SHARED on id 0, while the sort that reads it varies with `dir[d]` and
/// RECYCLES id 1. Values: the sort order of `[60, 20, 40]` ascending is
/// `[1, 2, 0]` and descending `[0, 2, 1]`; element 1 reads position 0 of the
/// ascending order, element 2 position 1 of the descending, element 3
/// position 2 of the ascending: `1, 2, 0`.
#[test]
fn a2a_per_element_hoist_over_a_shared_operand() {
    let project =
        fixture("a2a_top_per_elem").array_aux("out[d]", "VECTOR SORT ORDER(vals[*] * 2, dir[d])");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a per-element over an operand");
    assert_eq!(
        writes(&events),
        vec![0, 1, 1, 1],
        "one shared operand ahead of the elements, one recycled sort per element"
    );
    project.assert_vm_result("out", &[1.0, 2.0, 0.0]);
}

/// The same two regimes with the two temps in SIBLING terms rather than nested:
/// `SUM(vals[*] * 2)` is shared on id 0, the per-element sort recycles id 1.
/// Values: `120 + 3 = 123` for every element.
#[test]
fn a2a_per_element_hoist_beside_a_shared_operand() {
    let project = fixture("a2a_nested_per_elem").array_aux(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(VECTOR SORT ORDER(bump[*], dir[d]))",
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_dense_well_formed_and_sized(&events, &temp_sizes, "a2a nested per-element");
    assert_eq!(
        writes(&events),
        vec![0, 1, 1, 1],
        "one shared operand ahead of the elements, one recycled sort per element"
    );
    let read: BTreeSet<u32> = events
        .iter()
        .filter_map(|e| match e {
            TempEvent::Read { id } => Some(*id),
            TempEvent::Write { .. } => None,
        })
        .collect();
    assert_eq!(
        read,
        BTreeSet::from([0, 1]),
        "every temp the fragment writes is read: nothing is materialized and dropped"
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
    use crate::compiler::{ModuleCtx, VarRef};
    use crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting;

    let model_name = Ident::new("main");
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let var_sizes = crate::compiler::VarSizes::new();
    let tables = std::collections::HashMap::new();
    let dimensions: Vec<crate::dimensions::Dimension> = Vec::new();
    // The phase-invariant context `FragmentInput::emit_ctx` builds, spelled out
    // because this input has no `FragmentInput` -- it is the shape no lowering
    // produces.
    let base = ModuleCtx {
        ident: &model_name,
        inputs: &inputs,
        temp_sizes: &[],
        runlist_initials_by_var: &[],
        runlist_flows: &[],
        runlist_stocks: &[],
        var_sizes: &var_sizes,
        tables: &tables,
        dimensions: &dimensions,
    };
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
