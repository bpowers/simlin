// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Contract tests for a fragment's temp numbering (`ast::TempAllocator`).
//!
//! A fragment is one variable's one phase, the unit `compiler::Var::new`
//! lowers. `FragmentMerger`'s `Recycle` strategy max-merges fragment temp
//! `t` onto shared slot `t`, so fragment ids must be 0-based and dense.
//! Codegen sizes the temp region from `temp_sizes`, so every id must also
//! carry the largest view assigned to it.
//!
//! The production allocation surface is deliberately exhaustive and small:
//! `compiler::array_operand` allocates once for a computed array operand and
//! once for an array-valued result. The rows below cover both sites together,
//! sequential apply-to-all reuse, EXCEPT/explicit arrayed arms, and the
//! arrayed graphical-function result that has no `BuiltinSig::Array` result.
//! Builtin argument/result enumeration itself is covered by
//! `compiler::array_operand::tests::every_signature_row_has_an_explicit_materialization_policy`.
//!
//! Every production row checks three properties of the lowered `Vec<Expr>`:
//!
//! 1. ids written by `AssignTemp` are exactly `0..n`;
//! 2. every `TempArray` / `TempArrayElement` read is preceded by a write;
//! 3. emitted `temp_sizes` lists every id at its largest assigned view.
//!
//! Sequential assignments reuse one range because every temp produced for an
//! element is dead after that element's store. This makes a 300-element
//! equation consume the maximum concurrent temp count, not 300 distinct ids,
//! which keeps valid equations inside bytecode's `u8` `TempId` limit. Tests
//! assert simulation results whenever aliasing could change which value an
//! arm or element reads; exact write sequences are asserted only where reuse
//! and maximum-size accounting are the contract.
//!
//! Fixtures use the same `FragmentInput` constructor, lowerer, and emitter as
//! production. The last test deliberately hand-builds the impossible input it
//! documents: a gapped temp-id sequence, which the emitter must refuse.

use std::collections::{BTreeMap, BTreeSet};

use super::var_fragment::{ExplicitFragment, explicit_fragment_input};
use super::*;
use crate::compiler::fragment::lower_fragment;
use crate::compiler::{Expr, SubscriptIndex};
use crate::test_common::TestProject;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempEvent {
    Write { id: u32, size: usize },
    Read { id: u32 },
}

/// Events in VM evaluation order: an assignment's right-hand side before
/// the assignment, and builtin operands from left to right.
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
                        SubscriptIndex::Single(expr) => walk(expr, out),
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
            Expr::If(cond, then_expr, else_expr, _) => {
                walk(cond, out);
                walk(then_expr, out);
                walk(else_expr, out);
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

/// Lower and emit the non-initial phase through the production constructors.
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

    let exprs = match explicit_fragment_input(&db, source_var, model, source_project, &[]) {
        ExplicitFragment::Ready { input, .. } => {
            lower_fragment(&input, false)
                .unwrap_or_else(|e| panic!("`{var}` must lower: {e:?}"))
                .ast
        }
        ExplicitFragment::Fatal { fatal_diags, .. } => {
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

fn assert_temp_contract(events: &[TempEvent], temp_sizes: &[(u32, usize)], what: &str) {
    let mut written = BTreeSet::new();
    let mut max_size = BTreeMap::new();
    for event in events {
        match *event {
            TempEvent::Write { id, size } => {
                written.insert(id);
                max_size
                    .entry(id)
                    .and_modify(|old: &mut usize| *old = (*old).max(size))
                    .or_insert(size);
            }
            TempEvent::Read { id } => assert!(
                written.contains(&id),
                "{what}: temp {id} is read before any write; events {events:?}"
            ),
        }
    }
    let n = written.len() as u32;
    assert_eq!(
        written,
        (0..n).collect(),
        "{what}: written ids must be exactly 0..{n}; events {events:?}"
    );
    assert_eq!(
        temp_sizes,
        &max_size.into_iter().collect::<Vec<_>>(),
        "{what}: temp_sizes must record every id at its largest assigned view"
    );
}

fn writes(events: &[TempEvent]) -> Vec<(u32, usize)> {
    events
        .iter()
        .filter_map(|event| match event {
            TempEvent::Write { id, size } => Some((*id, *size)),
            TempEvent::Read { .. } => None,
        })
        .collect()
}

/// A computed reducer operand allocates id 0 and its pure resolved definition
/// is shared across the element assignments. The array-producing result uses
/// id 1 but contains that temp read, so each assignment writes it locally.
/// Both are concurrently live, so neither may alias the other.
#[test]
fn computed_operand_and_array_result_use_one_dense_range() {
    let project = fixture("operand_and_result").array_aux("out[d]", "RANK(vals[*] * 2, 1)");
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_temp_contract(&events, &temp_sizes, "operand and result");
    assert_eq!(writes(&events), vec![(0, 3), (1, 3), (1, 3), (1, 3)]);
    assert_eq!(temp_sizes, vec![(0, 3), (1, 3)]);
    project.assert_vm_result("out", &[3.0, 1.0, 2.0]);
}

/// Each apply-to-all element has the same two-temp peak. The invariant
/// computed operand's first write dominates the whole sequence, while the
/// array result varies with `dir[D]` and is written for each element. Reusing
/// ids across assignments keeps allocation proportional to expression depth
/// rather than array size.
#[test]
fn apply_to_all_elements_reuse_their_temp_range() {
    let project = fixture("a2a_reuse").array_aux(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(VECTOR SORT ORDER(bump[*], dir[d]))",
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_temp_contract(&events, &temp_sizes, "apply-to-all reuse");
    assert_eq!(writes(&events), vec![(0, 3), (1, 3), (1, 3), (1, 3)]);
    assert_eq!(temp_sizes, vec![(0, 3), (1, 3)]);
    project.assert_vm_result("out", &[123.0, 123.0, 123.0]);
}

/// Explicit and EXCEPT arms run sequentially and therefore share the same
/// range. A slot's emitted size is the maximum of all views assigned to it.
#[test]
fn arrayed_arms_reuse_ids_and_keep_the_largest_size() {
    let project = fixture("arrayed_reuse").array_with_ranges(
        "out[e]",
        vec![
            ("1", "SUM(matrix[*, 1] * 2)"),
            ("2", "SUM(vals[*] + bump[*] + 1)"),
        ],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_temp_contract(&events, &temp_sizes, "arrayed reuse");
    assert_eq!(writes(&events), vec![(0, 2), (0, 3)]);
    assert_eq!(temp_sizes, vec![(0, 3)]);
    project.assert_vm_result("out", &[22.0, 163.0]);

    let project = fixture("except_reuse").array_with_default_and_overrides(
        "out[d]",
        "SUM(vals[*] * 2) + SUM(RANK(bump[*], 1))",
        vec![("1", "SUM(RANK(vals[*], 1))")],
    );
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    assert_temp_contract(&temp_events(&exprs), &temp_sizes, "EXCEPT reuse");
    project.assert_vm_result("out", &[6.0, 126.0, 126.0]);
}

/// Arrayed graphical-function application is scalar in the signature table,
/// but lowering gives its table argument a retained view and materializes the
/// `LookupArray` result. This is the only result allocation path not enumerated
/// by `ResultKind::Array`.
#[test]
fn arrayed_graphical_function_results_reuse_one_slot() {
    let mdl = "{UTF-8}\n\
        COP: a, b, c ~~|\n\
        ROW: p, q ~~|\n\
        g[a,p]( (0,0),(1,10) ) ~~|\n\
        g[a,q]( (0,0),(1,20) ) ~~|\n\
        g[b,p]( (0,0),(1,30) ) ~~|\n\
        g[b,q]( (0,0),(1,40) ) ~~|\n\
        g[c,p]( (0,0),(1,50) ) ~~|\n\
        g[c,q]( (0,0),(1,60) ) ~~|\n\
        out[COP] = SUM(LOOKUP(g, Time)) ~~|\n\
        INITIAL TIME = 0 ~~|\n\
        FINAL TIME = 1 ~~|\n\
        SAVEPER = 1 ~~|\n\
        TIME STEP = 1 ~~|\n";
    let project = TestProject::from_datamodel(crate::open_vensim(mdl).expect("fixture parses"));
    let (exprs, temp_sizes) = flow_fragment(&project, "out");
    let events = temp_events(&exprs);
    assert_temp_contract(&events, &temp_sizes, "arrayed graphical function");
    assert_eq!(writes(&events), vec![(0, 2); 3]);
    assert_eq!(temp_sizes, vec![(0, 2)]);
    project.assert_vm_result("out", &[30.0, 70.0, 110.0]);
}

/// The emitter indexes the temp-size table by id. Production lowering cannot
/// make a gap, so this is deliberately hand-built to pin its defensive error.
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
