// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for `combine_scc_fragment` -- the per-element-granular
//! generalization of `concatenate_fragments` that interleaves a
//! multi-member recurrence SCC's per-element symbolic segments into one
//! combined `PerVarBytecodes` following `ResolvedScc.element_order`.
//!
//! Lives in its own file alongside the production code in `db.rs` to keep
//! both `db.rs` and `db/tests.rs` under the per-file line cap (same
//! convention as `db/dep_graph_tests.rs`).
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
fn combined_fragment_keeps_disjoint_temp_ranges_across_interleaved_segments() {
    // #583 guard: the plain-phase `concatenate_fragments` RECYCLES fragment
    // temps into one identity pool (a fragment's temps die at its runlist
    // segment end, so two fragments' id-0 temps may share slot 0). That
    // recycle is UNSOUND for `combine_scc_fragment`: its per-element segments
    // INTERLEAVE per `element_order` (`ce[0] -> ecc[0] -> ce[1] -> ecc[1]`),
    // so `ce`'s and `ecc`'s temp live ranges OVERLAP -- they must NEVER share
    // a slot. This pins that `combine_scc_fragment` stays on the disjoint
    // (sum) temp path even after the plain-phase recycle lands.
    let frags = two_member_fragments(); // `ce` temp (0,4), `ecc` temp (0,8)
    let resolved = scc(interleaved_order());

    let combined = combine_scc_fragment(&resolved, &frags).expect("must combine");

    // Both members carry a fragment-local temp id 0. In the combined
    // fragment they MUST occupy DISTINCT ids (0 and 1), proving the SCC path
    // did not recycle them onto a shared slot.
    let mut ids: Vec<u32> = combined.temp_sizes.iter().map(|(tid, _)| *tid).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![0, 1],
        "two interleaved members' id-0 temps must get DISJOINT ids 0 and 1 \
         (not recycled to a shared slot) -- their live ranges overlap"
    );
    // The sizes stay per-member (NOT max-merged), since the slots are
    // distinct: `ce`'s 4 at id 0, `ecc`'s 8 at id 1.
    let sizes: HashMap<u32, usize> = combined.temp_sizes.iter().copied().collect();
    assert_eq!(sizes.get(&0), Some(&4), "ce's temp size at its own slot 0");
    assert_eq!(sizes.get(&1), Some(&8), "ecc's temp size at its own slot 1");
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

// ── AC2.3: combined-fragment injection is layout-transparent ────────────
//
// End-to-end through `assemble_module` (the Task 6 production consumer).
// For a resolved multi-variable recurrence SCC (`ref.mdl`-shaped
// `ce`/`ecc`), the assembled module's per-member write slots must be
// IDENTICAL to the slots a hypothetical acyclic equivalent gets -- i.e.
// exactly `compute_layout`'s element-slot range for each member.
// `compute_layout` is SCC-agnostic by construction (it assigns offsets
// purely by sorted name + size, never consulting `resolved_sccs`), so it
// IS the "hypothetical acyclic equivalent" offset map. The combined
// fragment keeps every write's original `SymVarRef { name,
// element_offset }`, so `resolve_module` maps each write to the same
// model slot it would get without the SCC -- per-variable result series
// stay individually addressable (AC2.3).

use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

/// A `ref.mdl`-shaped two-variable inter-element recurrence: whole-
/// variable `ce`<->`ecc` is a 2-cycle, but the induced element graph is
/// acyclic, so Task 4/5b resolve the `{ce,ecc}` SCC.
fn ref_shaped_project() -> TestProject {
    TestProject::new("ref_shaped_assemble")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ce[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        )
        .array_with_ranges(
            "ecc[t]",
            vec![
                ("t1", "ce[t1] + 1"),
                ("t2", "ce[t2] + 1"),
                ("t3", "ce[t3] + 1"),
            ],
        )
}

/// `compute_layout`'s contiguous element slot range for `name`.
fn layout_slots(layout: &crate::compiler::symbolic::VariableLayout, name: &str) -> Vec<usize> {
    let e = layout
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` must be in the layout"));
    (e.offset..e.offset + e.size).collect()
}

#[test]
fn assemble_module_resolved_scc_member_offsets_match_acyclic_layout() {
    let db = SimlinDb::default();
    let dm = ref_shaped_project().build_datamodel();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;
    let project = result.project;

    // Precondition (Task 5b): the multi-member SCC must be resolved and
    // the gate must NOT report a cycle, otherwise `assemble_module`
    // early-returns before injecting anything.
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "Task 5b precondition: the element-acyclic {{ce,ecc}} SCC must \
         survive the cycle gate (has_cycle == false)"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one resolved SCC ({{ce,ecc}}) -- got {:?}",
        dep_graph.resolved_sccs
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [Ident::new("ce"), Ident::new("ecc")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(dep_graph.resolved_sccs[0].phase, SccPhase::Dt);

    // The SCC-agnostic "hypothetical acyclic equivalent" offset map. The
    // module is assembled as root below, so compare against the root-shifted
    // layout (the same one `assemble_module`'s root path resolves against):
    // `compute_layout` now returns the body layout (0-based) and the root
    // shift relocates every entry by `IMPLICIT_VAR_COUNT`.
    let layout = crate::db::compute_layout(&db, model, project).root_shifted();
    let mut acyclic_ce = layout_slots(&layout, "ce");
    let mut acyclic_ecc = layout_slots(&layout, "ecc");
    acyclic_ce.sort_unstable();
    acyclic_ecc.sort_unstable();
    assert_eq!(acyclic_ce.len(), 3, "ce occupies 3 element slots");
    assert_eq!(acyclic_ecc.len(), 3, "ecc occupies 3 element slots");

    // Assemble the module: Task 6 must inject the combined `{ce,ecc}`
    // fragment into the flows phase (skipping the per-variable pushes),
    // its writes keeping their original `(name, element_offset)`.
    let module = crate::db::assemble_module(
        &db,
        model,
        project,
        true,
        crate::db::ModuleInputSet::empty(&db),
    )
    .expect("ref-shaped resolved SCC must assemble (no CircularDependency)");

    // The assembled flows bytecode's AssignCurr target offsets, re-derived
    // from the resolved bytecode exactly as `resolve_module` does.
    let flow_offsets =
        crate::compiler::symbolic::extract_assign_curr_offsets(&module.compiled_flows);

    // AC2.3: every member element slot the acyclic layout assigns is
    // written by the combined fragment, at EXACTLY that slot -- the
    // combined fragment is layout-transparent (it neither moves a write
    // off its layout slot nor drops/duplicates a member element).
    for slot in acyclic_ce.iter().chain(acyclic_ecc.iter()) {
        assert!(
            flow_offsets.contains(slot),
            "AC2.3: the combined SCC fragment must write member slot \
             {slot} (ce slots {acyclic_ce:?}, ecc slots {acyclic_ecc:?}); \
             assembled flow AssignCurr offsets = {flow_offsets:?}"
        );
    }

    // The combined fragment writes ONLY the six member slots for the SCC
    // (no extra/foreign slot, no perturbation). `ce`/`ecc` are the only
    // flow variables here, so the assembled flows' write set is exactly
    // the union of the two members' acyclic slot ranges -- proving the
    // resolution did not shift any other variable's offsets either.
    let mut expected: Vec<usize> = acyclic_ce
        .iter()
        .chain(acyclic_ecc.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(
        flow_offsets, expected,
        "AC2.3: the assembled flows must write exactly the acyclic \
         layout's {{ce,ecc}} slots -- no offset perturbation"
    );

    // Task 6 behavioral signature (RED before the injection, GREEN
    // after): the combined fragment is injected, so the assembled flows
    // bytecode emits the member writes in the SCC's INTERLEAVED
    // `element_order` -- NOT as two per-variable contiguous blocks. For
    // this `ref.mdl` shape the verdict order is
    //   ce[t1] (const) -> ecc[t1]=ce[t1]+1 -> ce[t2]=ecc[t1]+1 ->
    //   ecc[t2]=ce[t2]+1 -> ce[t3]=ecc[t2]+1 -> ecc[t3]=ce[t3]+1,
    // i.e. interleaved absolute slots
    //   [ce0, ecc0, ce1, ecc1, ce2, ecc2].
    // Before Task 6 each member is pushed as its own fragment, so the
    // order is the per-variable contiguous [ce0,ce1,ce2, ecc0,ecc1,ecc2]
    // -- this assertion FAILS RED until the combined fragment is
    // injected at the first SCC member's runlist slot.
    let ordered_writes: Vec<usize> = module
        .compiled_flows
        .code
        .iter()
        .filter_map(|op| match op {
            crate::bytecode::Opcode::AssignCurr { off }
            | crate::bytecode::Opcode::AssignConstCurr { off, .. }
            | crate::bytecode::Opcode::BinOpAssignCurr { off, .. } => Some(*off as usize),
            _ => None,
        })
        .collect();
    let interleaved = vec![
        acyclic_ce[0],
        acyclic_ecc[0],
        acyclic_ce[1],
        acyclic_ecc[1],
        acyclic_ce[2],
        acyclic_ecc[2],
    ];
    assert_eq!(
        ordered_writes, interleaved,
        "Task 6: the assembled flows must emit member writes in the SCC's \
         interleaved element_order (combined fragment injected), not as \
         two per-variable contiguous blocks. element_order = {:?}",
        dep_graph.resolved_sccs[0].element_order
    );
}

// ── AC2.3 (Phase 2 Task 10): byte-stable combined fragment ──────────────
//
// `combined_fragment_is_byte_stable` (above) pins determinism of the
// isolated `combine_scc_fragment` builder on HAND-BUILT inputs. This is
// the PRODUCTION-PAYLOAD obligation: the *assembled* combined fragment
// (the bytecode that actually rides on the compiled module and drives
// the VM), the emitted `resolved_sccs`, and each SCC's `element_order`
// must be byte-identical across two independent compiles on FRESH
// databases (no HashMap-iteration nondeterminism leaking through the
// identification -> symbolic verdict -> interleave -> assemble path).
// A regression that leaked iteration order anywhere on that pipeline
// would fail here even if the isolated builder stayed stable. Modeled on
// the existing `model_dependency_graph_resolved_sccs_is_byte_stable_
// across_runs` discipline, extended to the assembled bytecode.

/// Two arrayed stocks `cs[t]` / `ecs[t]` whose per-element INTEG initial
/// values form a `ref.mdl`-shaped inter-element recurrence ACROSS the two
/// variables, with a constant zero inflow `g`. Each stock breaks the dt
/// chain (its dt-equation is the acyclic flow `g`), so the only cycle is
/// the MULTI-member INIT recurrence `{cs,ecs}` -- exercising the
/// synthetic-ident `SymbolicCompiledInitial` combined-init-fragment path
/// (Task 6). Mirrors the `two_stock_init_recurrence_project` shape
/// empirically confirmed in `db/dep_graph_tests.rs`; kept self-contained
/// here so the combined-fragment determinism coverage lives in one file.
fn two_stock_init_recurrence_datamodel() -> crate::datamodel::Project {
    use crate::datamodel::{self, Dimension, Equation, Flow, Stock, Variable};
    let dims = vec!["t".to_string()];
    let arrayed = |eqs: Vec<(&str, &str)>| {
        Equation::Arrayed(
            dims.clone(),
            eqs.into_iter()
                .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
                .collect(),
            None,
            false,
        )
    };
    let stock = |ident: &str, eq: Equation| {
        Variable::Stock(Stock {
            ident: ident.to_string(),
            equation: eq,
            documentation: String::new(),
            units: None,
            inflows: vec!["g".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![Dimension::named(
            "t".to_string(),
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                stock(
                    "cs",
                    arrayed(vec![
                        ("t1", "1"),
                        ("t2", "ecs[t1] + 1"),
                        ("t3", "ecs[t2] + 1"),
                    ]),
                ),
                stock(
                    "ecs",
                    arrayed(vec![
                        ("t1", "cs[t1] + 1"),
                        ("t2", "cs[t2] + 1"),
                        ("t3", "cs[t3] + 1"),
                    ]),
                ),
                Variable::Flow(Flow {
                    ident: "g".to_string(),
                    equation: Equation::ApplyToAll(dims, "0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Build the combined `PerVarBytecodes` for the single resolved SCC of
/// `dm`'s `main` model, on a FRESH database, via the EXACT production
/// path `assemble_module` uses (`var_phase_symbolic_fragment_prod` per
/// member -> `combine_scc_fragment`). Returns the emitted `resolved_sccs`
/// and the combined fragment. `assemble_module`'s `compiled_flows` /
/// `compiled_initials` are lowered `Opcode` streams (no `PartialEq`); the
/// combined fragment is byte-comparable at THIS symbolic layer (the layer
/// at which it is actually constructed and injected -- `PerVarBytecodes`
/// derives `PartialEq`), so this is the precise determinism probe for the
/// interleave + per-member resource renumber.
fn fresh_resolved_scc_and_combined_fragment(
    dm: &crate::datamodel::Project,
) -> (Vec<ResolvedScc>, crate::compiler::symbolic::PerVarBytecodes) {
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, dm);
    let model = result.models["main"].source;
    let project = result.project;
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "fixture must resolve exactly one SCC (got {:?})",
        dep_graph.resolved_sccs
    );
    let scc = &dep_graph.resolved_sccs[0];
    // Mirror `assemble_module`'s `combine_scc_for_phase` exactly: each
    // member's production symbolic fragment for the SCC's phase, then the
    // per-element-granular interleave.
    let mut member_fragments: HashMap<
        Ident<Canonical>,
        crate::compiler::symbolic::PerVarBytecodes,
    > = HashMap::with_capacity(scc.members.len());
    for member in &scc.members {
        let frag = crate::db::var_phase_symbolic_fragment_prod(
            &db,
            model,
            project,
            member.as_str(),
            scc.phase.clone(),
        )
        .unwrap_or_else(|| {
            panic!(
                "member `{}` must have a sourceable fragment",
                member.as_str()
            )
        });
        member_fragments.insert(member.clone(), frag);
    }
    let combined = combine_scc_fragment(scc, &member_fragments)
        .expect("the resolved SCC must combine into one fragment");
    (dep_graph.resolved_sccs.clone(), combined)
}

#[test]
fn assembled_dt_combined_fragment_is_byte_stable_across_fresh_dbs() {
    // The DT combined fragment (`ref.mdl`-shaped `{ce,ecc}`): build it
    // TWICE on independent fresh databases via the exact production path
    // and assert the emitted `resolved_sccs` (members + element_order +
    // phase) AND the combined `PerVarBytecodes` (the bytecode that is
    // injected into the flows phase) are byte-identical. A
    // nondeterministic interleave or per-member resource renumber would
    // surface as a fragment diff here.
    let dm = ref_shaped_project().build_datamodel();
    let (sccs_a, frag_a) = fresh_resolved_scc_and_combined_fragment(&dm);
    let (sccs_b, frag_b) = fresh_resolved_scc_and_combined_fragment(&dm);

    // Non-vacuous: the payload is the resolved `{ce,ecc}` dt SCC.
    assert_eq!(sccs_a[0].phase, SccPhase::Dt);
    assert_eq!(
        sccs_a[0].element_order.len(),
        6,
        "the dt SCC element_order must be the 6 interleaved (member,elem) \
         pairs (got {:?})",
        sccs_a[0].element_order
    );
    assert_eq!(
        sccs_a, sccs_b,
        "the emitted resolved_sccs (members + element_order + phase) must \
         be byte-identical across two fresh-DB compiles"
    );
    assert_eq!(
        frag_a, frag_b,
        "the assembled combined DT fragment (PerVarBytecodes: symbolic \
         code + literals + all side-channels) must be byte-identical \
         across two fresh-DB compiles -- a nondeterministic interleave / \
         per-member resource renumber would diff here (AC2.3)"
    );
}

#[test]
fn assembled_init_combined_fragment_is_byte_stable_across_fresh_dbs() {
    // The INIT combined fragment (the MULTI-member `{cs,ecs}` init
    // recurrence behind stocks -- AC2.4's synthetic-ident
    // `SymbolicCompiledInitial` path): build it TWICE on fresh databases
    // and assert the emitted `resolved_sccs` (phase Initial) and the
    // combined init `PerVarBytecodes` are byte-identical. This pins
    // determinism of the init combined-fragment construction
    // specifically (a distinct phase from the dt flows path).
    let dm = two_stock_init_recurrence_datamodel();
    let (sccs_a, frag_a) = fresh_resolved_scc_and_combined_fragment(&dm);
    let (sccs_b, frag_b) = fresh_resolved_scc_and_combined_fragment(&dm);

    assert_eq!(
        sccs_a[0].phase,
        SccPhase::Initial,
        "the resolved SCC must be the init-phase {{cs,ecs}} recurrence"
    );
    assert!(
        sccs_a[0].members.len() >= 2,
        "AC2.4: the init SCC must be MULTI-member (got {:?})",
        sccs_a[0].members
    );
    assert_eq!(
        sccs_a, sccs_b,
        "the emitted init resolved_sccs (members + element_order + phase) \
         must be byte-identical across two fresh-DB compiles"
    );
    assert_eq!(
        frag_a, frag_b,
        "the assembled combined INIT fragment (PerVarBytecodes, the one \
         the synthetic-ident SymbolicCompiledInitial carries) must be \
         byte-identical across two fresh-DB compiles (AC2.3 determinism, \
         init path)"
    );
}
