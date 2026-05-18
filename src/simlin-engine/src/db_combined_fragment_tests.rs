// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for `combine_scc_fragment` -- the per-element-granular
//! generalization of `concatenate_fragments` that interleaves a
//! multi-member recurrence SCC's per-element symbolic segments into one
//! combined `PerVarBytecodes` following `ResolvedScc.element_order`.
//!
//! Lives in its own file alongside the production code in `db.rs` to keep
//! both `db.rs` and `db_tests.rs` under the per-file line cap (same
//! convention as `db_dep_graph_tests.rs`).
//!
//! These tests are intentionally focused on STRUCTURAL well-formedness:
//! segment ordering, write-ref identity preservation, the single trailing
//! `Ret`, per-member resource renumbering with no cross-member collision,
//! and merged side-channels. Numeric correctness is the end-to-end job of
//! the `ref.mdl` / `interleaved.mdl` simulation tests (Tasks 7/8).

use super::combine_scc_fragment;
use crate::common::{Canonical, Ident};
use crate::compiler::symbolic::{
    PerVarBytecodes, SymStaticViewBase, SymVarRef, SymbolicByteCode, SymbolicModuleDecl,
    SymbolicOpcode, SymbolicStaticView,
};
use crate::db::{ResolvedScc, SccPhase};
use smallvec::SmallVec;
use std::collections::{BTreeSet, HashMap};

fn id(s: &str) -> Ident<Canonical> {
    Ident::new(s)
}

fn vref(name: &str, element_offset: usize) -> SymVarRef {
    SymVarRef {
        name: name.to_string(),
        element_offset,
    }
}

/// A two-member `ref.mdl`-shaped SCC: `ce` and `ecc`, each over a
/// 2-element subrange. Each member's symbolic fragment is a flat sequence
/// of per-element computations, each terminated by that element's write
/// opcode (`AssignConstCurr` / `AssignCurr` with `var.name == member`),
/// followed by a single trailing `Ret`. Each member also carries one
/// graphical function, one module decl, one static view (base = a member
/// variable, so it must survive verbatim), one temp, and one dim list, so
/// the per-member resource renumbering is exercised.
fn two_member_fragments() -> HashMap<Ident<Canonical>, PerVarBytecodes> {
    // `ce`: ce[0] = const(lit 0); ce[1] = LoadVar(ecc[0]) (so it reads
    // member `ecc`'s element 0). Side-channels at fragment-local id 0.
    let ce = PerVarBytecodes {
        symbolic: SymbolicByteCode {
            literals: vec![10.0],
            code: vec![
                // ce[0] = 10.0
                SymbolicOpcode::AssignConstCurr {
                    var: vref("ce", 0),
                    literal_id: 0,
                },
                // ce[1] = ecc[0]
                SymbolicOpcode::LoadVar {
                    var: vref("ecc", 0),
                },
                SymbolicOpcode::AssignCurr { var: vref("ce", 1) },
                SymbolicOpcode::Ret,
            ],
        },
        graphical_functions: vec![vec![(0.0, 0.0), (1.0, 1.0)]],
        module_decls: vec![SymbolicModuleDecl {
            model_name: id("modce"),
            input_set: BTreeSet::new(),
            var: vref("ce", 0),
        }],
        static_views: vec![SymbolicStaticView {
            base: SymStaticViewBase::Var(vref("ce", 0)),
            dims: SmallVec::new(),
            strides: SmallVec::new(),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
        }],
        temp_sizes: vec![(0, 4)],
        dim_lists: vec![vec![2]],
    };

    // `ecc`: ecc[0] = LoadVar(ce[0]); ecc[1] = const(lit 0). Side-channels
    // also at fragment-local id 0 -- they MUST be renumbered so they do
    // not collide with `ce`'s id-0 resources in the combined fragment.
    let ecc = PerVarBytecodes {
        symbolic: SymbolicByteCode {
            literals: vec![20.0],
            code: vec![
                // ecc[0] = ce[0]
                SymbolicOpcode::LoadVar { var: vref("ce", 0) },
                SymbolicOpcode::AssignCurr {
                    var: vref("ecc", 0),
                },
                // ecc[1] = 20.0
                SymbolicOpcode::AssignConstCurr {
                    var: vref("ecc", 1),
                    literal_id: 0,
                },
                SymbolicOpcode::Ret,
            ],
        },
        graphical_functions: vec![vec![(2.0, 2.0), (3.0, 3.0)]],
        module_decls: vec![SymbolicModuleDecl {
            model_name: id("modecc"),
            input_set: BTreeSet::new(),
            var: vref("ecc", 0),
        }],
        static_views: vec![SymbolicStaticView {
            base: SymStaticViewBase::Var(vref("ecc", 1)),
            dims: SmallVec::new(),
            strides: SmallVec::new(),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
        }],
        temp_sizes: vec![(0, 8)],
        dim_lists: vec![vec![2]],
    };

    let mut m = HashMap::new();
    m.insert(id("ce"), ce);
    m.insert(id("ecc"), ecc);
    m
}

/// The interleaved `element_order` an element-acyclic verdict would
/// produce for the fragments above:
/// `ce[0] -> ecc[0] -> ce[1] -> ecc[1]`.
fn interleaved_order() -> Vec<(Ident<Canonical>, usize)> {
    vec![(id("ce"), 0), (id("ecc"), 0), (id("ce"), 1), (id("ecc"), 1)]
}

fn scc(order: Vec<(Ident<Canonical>, usize)>) -> ResolvedScc {
    let members: BTreeSet<Ident<Canonical>> = order.iter().map(|(m, _)| m.clone()).collect();
    ResolvedScc {
        members,
        element_order: order,
        phase: SccPhase::Dt,
    }
}

/// Collect the ordered list of per-element write `SymVarRef`s in a
/// combined fragment (the writes whose name is one of the SCC members).
fn write_refs(bc: &PerVarBytecodes) -> Vec<SymVarRef> {
    bc.symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::AssignCurr { var }
            | SymbolicOpcode::AssignConstCurr { var, .. }
            | SymbolicOpcode::BinOpAssignCurr { var, .. } => Some(var.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn combined_fragment_emits_writes_in_element_order() {
    let frags = two_member_fragments();
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("well-formed SCC must combine");

    // The per-element writes must appear in exactly `element_order`, each
    // keeping its ORIGINAL `(name, element_offset)` (only segment order
    // changes -- so `resolve_module` later maps to the same model slots
    // and per-variable series stay individually addressable, AC2.3).
    let writes = write_refs(&combined);
    assert_eq!(
        writes,
        vec![vref("ce", 0), vref("ecc", 0), vref("ce", 1), vref("ecc", 1),],
        "combined writes must follow element_order with original refs"
    );
}

#[test]
fn combined_fragment_has_exactly_one_trailing_ret() {
    let frags = two_member_fragments();
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");

    let ret_count = combined
        .symbolic
        .code
        .iter()
        .filter(|op| matches!(op, SymbolicOpcode::Ret))
        .count();
    assert_eq!(ret_count, 1, "exactly one Ret in the combined fragment");
    assert_eq!(
        combined.symbolic.code.last(),
        Some(&SymbolicOpcode::Ret),
        "the single Ret must be the final opcode"
    );
}

#[test]
fn combined_fragment_each_member_write_present_exactly_once() {
    let frags = two_member_fragments();
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");
    let writes = write_refs(&combined);

    for member in ["ce", "ecc"] {
        for elem in 0..2 {
            let want = vref(member, elem);
            let n = writes.iter().filter(|w| **w == want).count();
            assert_eq!(
                n, 1,
                "{member}[{elem}] must be written exactly once, found {n}"
            );
        }
    }
}

#[test]
fn combined_fragment_renumbers_resources_per_member_no_collision() {
    let frags = two_member_fragments();
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");

    // Member offsets are assigned in `element_order` first-encounter
    // order: `ce` first (resources at base 0), then `ecc` (resources
    // after `ce`'s). `ce` and `ecc` each had ONE gf / module / view /
    // temp / dim_list at fragment-local id 0; merged they must be two
    // distinct, non-colliding entries.
    assert_eq!(
        combined.graphical_functions.len(),
        2,
        "two members' GFs merged"
    );
    assert_eq!(
        combined.graphical_functions[0],
        vec![(0.0, 0.0), (1.0, 1.0)],
        "ce's GF first (member first-encounter order)"
    );
    assert_eq!(
        combined.graphical_functions[1],
        vec![(2.0, 2.0), (3.0, 3.0)],
        "ecc's GF second"
    );
    assert_eq!(combined.module_decls.len(), 2, "two members' modules");
    assert_eq!(combined.module_decls[0].model_name, id("modce"));
    assert_eq!(combined.module_decls[1].model_name, id("modecc"));
    assert_eq!(combined.static_views.len(), 2, "two members' views");
    assert_eq!(combined.dim_lists.len(), 2, "two members' dim lists");

    // Literals: phase-local per fragment, concatenated in member order.
    assert_eq!(
        combined.symbolic.literals,
        vec![10.0, 20.0],
        "ce's literal pool then ecc's"
    );

    // `ce`'s `AssignConstCurr` keeps literal_id 0 (member base 0); `ecc`'s
    // `AssignConstCurr` must be renumbered to literal_id 1 (after ce's
    // single literal). This proves per-member literal renumbering.
    let const_assigns: Vec<(SymVarRef, u16)> = combined
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::AssignConstCurr { var, literal_id } => Some((var.clone(), *literal_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        const_assigns,
        vec![(vref("ce", 0), 0), (vref("ecc", 1), 1)],
        "ce's const-assign at literal 0, ecc's renumbered to literal 1"
    );

    // The temp count must be the sum of the per-member temp counts (one
    // each), proving the temp namespace does not collide either.
    let temp_max = combined
        .temp_sizes
        .iter()
        .map(|(tid, _)| *tid + 1)
        .max()
        .unwrap_or(0);
    assert_eq!(
        temp_max, 2,
        "two members' single temps occupy distinct ids 0 and 1"
    );
}

#[test]
fn combined_fragment_static_view_var_base_survives_verbatim() {
    let frags = two_member_fragments();
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");

    // A static view whose base is a model variable must keep its
    // `SymVarRef` verbatim (only `Temp` bases get a temp_offset).
    let bases: Vec<SymStaticViewBase> = combined
        .static_views
        .iter()
        .map(|v| v.base.clone())
        .collect();
    assert_eq!(
        bases,
        vec![
            SymStaticViewBase::Var(vref("ce", 0)),
            SymStaticViewBase::Var(vref("ecc", 1)),
        ],
        "Var-based static views survive the merge unchanged"
    );
}

#[test]
fn combined_fragment_member_first_encounter_order_drives_resources() {
    // `element_order` whose FIRST member is `ecc`, not `ce`. The
    // per-member resource offsets are assigned in element_order's member
    // first-encounter order, so here `ecc`'s resources come first.
    let order = vec![(id("ecc"), 0), (id("ce"), 0), (id("ce"), 1), (id("ecc"), 1)];
    let frags = two_member_fragments();
    let resolved = scc(order.clone());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");

    // Writes still follow element_order exactly.
    assert_eq!(
        write_refs(&combined),
        vec![vref("ecc", 0), vref("ce", 0), vref("ce", 1), vref("ecc", 1),]
    );
    // ecc encountered first => ecc's GF / literals come first now.
    assert_eq!(
        combined.graphical_functions[0],
        vec![(2.0, 2.0), (3.0, 3.0)],
        "ecc's GF first (it is the first member in element_order)"
    );
    assert_eq!(
        combined.symbolic.literals,
        vec![20.0, 10.0],
        "ecc's literal pool first, then ce's"
    );
    // Now ecc's const-assign is at literal 0, ce's renumbered to 1.
    let const_assigns: Vec<(SymVarRef, u16)> = combined
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::AssignConstCurr { var, literal_id } => Some((var.clone(), *literal_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        const_assigns,
        vec![(vref("ce", 0), 1), (vref("ecc", 1), 0)],
        "ecc base 0 / ce base 1 -- offsets follow first-encounter order"
    );
}

#[test]
fn combined_fragment_is_byte_stable() {
    // The interleave is a pure reordering with deterministic per-member
    // offset assignment, so two independent combines of the same inputs
    // are byte-identical (AC2.3 determinism).
    let resolved = scc(interleaved_order());
    let a = combine_scc_fragment(&resolved, &two_member_fragments()).unwrap();
    let b = combine_scc_fragment(&resolved, &two_member_fragments()).unwrap();
    assert_eq!(a, b, "combined PerVarBytecodes must be byte-stable");
}

// ── Loud-safe error path (defense-in-depth) ─────────────────────────────
//
// A malformed member fragment that is missing a segment for an element
// named in `element_order` must surface as an `Err` -- NEVER a panic and
// NEVER a silently-malformed combined fragment. The caller keeps
// `CircularDependency`.

#[test]
fn combined_fragment_missing_element_segment_is_loud_safe_err() {
    let mut frags = two_member_fragments();
    // Corrupt `ecc`: drop its element-1 write entirely, so the segment
    // for `ecc[1]` (which `element_order` requires) does not exist.
    let ecc = frags.get_mut(&id("ecc")).unwrap();
    ecc.symbolic.code = vec![
        SymbolicOpcode::LoadVar { var: vref("ce", 0) },
        SymbolicOpcode::AssignCurr {
            var: vref("ecc", 0),
        },
        // ecc[1] write deliberately omitted.
        SymbolicOpcode::Ret,
    ];
    let resolved = scc(interleaved_order());

    let result = combine_scc_fragment(&resolved, &frags);
    assert!(
        result.is_err(),
        "a member missing an element segment must be a loud-safe Err, \
         got {result:?}"
    );
}

#[test]
fn combined_fragment_duplicate_element_segment_is_loud_safe_err() {
    let mut frags = two_member_fragments();
    // Corrupt `ce`: write element 0 twice. A duplicate segment for the
    // same element is ambiguous -> loud-safe Err (never pick one).
    let ce = frags.get_mut(&id("ce")).unwrap();
    ce.symbolic.code = vec![
        SymbolicOpcode::AssignConstCurr {
            var: vref("ce", 0),
            literal_id: 0,
        },
        SymbolicOpcode::AssignConstCurr {
            var: vref("ce", 0),
            literal_id: 0,
        },
        SymbolicOpcode::LoadVar {
            var: vref("ecc", 0),
        },
        SymbolicOpcode::AssignCurr { var: vref("ce", 1) },
        SymbolicOpcode::Ret,
    ];
    let resolved = scc(interleaved_order());

    let result = combine_scc_fragment(&resolved, &frags);
    assert!(
        result.is_err(),
        "a member with a duplicate element segment must be a loud-safe \
         Err, got {result:?}"
    );
}

#[test]
fn combined_fragment_member_fragment_absent_is_loud_safe_err() {
    let mut frags = two_member_fragments();
    // The Task 4 accessor returned `None` for `ecc` (unsourceable); the
    // caller could not supply its fragment. The combiner must not panic
    // on the missing map entry -- it must be a loud-safe Err.
    frags.remove(&id("ecc"));
    let resolved = scc(interleaved_order());

    let result = combine_scc_fragment(&resolved, &frags);
    assert!(
        result.is_err(),
        "an SCC member with no supplied fragment must be a loud-safe Err, \
         got {result:?}"
    );
}

#[test]
fn combined_fragment_trailing_non_write_opcodes_join_last_segment() {
    // A member whose final element write is followed by trailing
    // non-write opcodes (before the `Ret`). Those trailing opcodes belong
    // to the last element's segment (mirrors the Task 4 segmentation
    // contract: a segment is the run UP TO AND INCLUDING the write, but a
    // tail after the final write must not be silently dropped -- it would
    // change semantics). Here the tail is harmless (a `PopView`); the
    // contract we assert is that the combine still succeeds and the write
    // ordering is correct (no opcode loss is separately covered by the
    // numeric end-to-end tests).
    let mut frags = two_member_fragments();
    let ce = frags.get_mut(&id("ce")).unwrap();
    ce.symbolic.code = vec![
        SymbolicOpcode::AssignConstCurr {
            var: vref("ce", 0),
            literal_id: 0,
        },
        SymbolicOpcode::LoadVar {
            var: vref("ecc", 0),
        },
        SymbolicOpcode::AssignCurr { var: vref("ce", 1) },
        // Trailing non-write opcode after the last write, before Ret.
        SymbolicOpcode::PopView {},
        SymbolicOpcode::Ret,
    ];
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags)
        .expect("a trailing non-write tail must not break combination");
    assert_eq!(
        write_refs(&combined),
        vec![vref("ce", 0), vref("ecc", 0), vref("ce", 1), vref("ecc", 1),]
    );
}
