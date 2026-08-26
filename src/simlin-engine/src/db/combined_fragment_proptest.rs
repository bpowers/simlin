// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property tests for `combine_scc_fragment` -- the INTERLEAVING consumer of
//! `FragmentMerger`, and therefore the other half of obligations M5 and M6.
//!
//! `compiler::symbolic_merge_proptest` covers the sequential consumer, where
//! each fragment's opcodes are one contiguous run. Everything that makes that
//! emission safe changes here: a resolved recurrence SCC's members are cut into
//! per-element segments and re-emitted in `element_order`, so `ce[0]` runs,
//! then `ecc[0]`, then `ce[1]`. Members' temp live ranges therefore OVERLAP,
//! and the two properties this file adds are exactly the ones that overlap
//! makes non-trivial:
//!
//! * **M5** -- interleaved members must never share a temp slot. In the
//!   sequential case sharing is safe and necessary; here it would alias two
//!   simultaneously-live scratch arrays and silently miscompile the SCC. Stated
//!   as the same non-interleaving condition the sequential property uses, so
//!   the two are the same rule applied to two emission shapes rather than two
//!   different beliefs.
//!
//! * **M6** -- the interleave is a pure REORDERING. Every member opcode
//!   survives exactly once, each per-element write keeps its original
//!   `SymVarRef` (which is what lets `resolve_module` place it at the same slot
//!   the acyclic layout would, AC2.3), and the writes come out in
//!   `element_order`.
//!
//! The third obligation the reordering creates -- that a relative backward jump
//! must not leave its own segment -- is pinned by hand-built fixtures in
//! `combined_fragment_tests.rs` instead, beside that file's other loud-safe
//! segmentation tests: the shape needs an exactly-placed jump, which a
//! generator would only reach by accident.
//!
//! `combined_fragment_tests.rs` pins the same contract on two hand-built
//! `ref.mdl`-shaped members. These properties widen it to arbitrary member
//! counts, element counts, resource loads and interleave orders -- the axes a
//! two-member fixture cannot vary.

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use super::combine_scc_fragment;
use crate::common::{Canonical, Ident};
use crate::compiler::symbolic::{
    PerVarBytecodes, SymStaticViewBase, SymVarRef, SymbolicByteCode, SymbolicModuleDecl,
    SymbolicOpcode, SymbolicStaticView,
};
use crate::db::{ResolvedScc, SccPhase};
use smallvec::SmallVec;
use std::collections::{BTreeSet, HashMap, HashSet};

/// One SCC member: `n_elements` per-element segments, each carrying a slice of
/// the member's resources and terminated by that element's write.
#[derive(Clone, Debug)]
struct MemberSpec {
    /// 0-based index; the member is named `m{index}`.
    index: usize,
    n_elements: usize,
    n_literals: usize,
    n_temps: usize,
    n_modules: usize,
    n_views: usize,
    n_gf_tables: usize,
    /// Emit a self-contained iteration loop inside the first element's
    /// segment, so the properties see a backward jump.
    with_loop: bool,
}

fn member_spec() -> impl Strategy<Value = MemberSpec> {
    (
        1usize..4, // n_elements
        0usize..3, // n_literals
        0usize..3, // n_temps
        0usize..2, // n_modules
        0usize..3, // n_views
        0usize..3, // n_gf_tables
        any::<bool>(),
    )
        .prop_map(
            |(n_elements, n_literals, n_temps, n_modules, n_views, n_gf_tables, with_loop)| {
                MemberSpec {
                    index: 0,
                    n_elements,
                    n_literals,
                    n_temps,
                    n_modules,
                    n_views,
                    n_gf_tables,
                    with_loop,
                }
            },
        )
}

/// Two to four members, indexed so their names and resource values differ.
fn member_specs() -> impl Strategy<Value = Vec<MemberSpec>> {
    prop::collection::vec(member_spec(), 2..5).prop_map(|mut specs| {
        for (i, spec) in specs.iter_mut().enumerate() {
            spec.index = i;
        }
        specs
    })
}

fn member_name(index: usize) -> Ident<Canonical> {
    Ident::new(&format!("m{index}"))
}

fn vref(name: &Ident<Canonical>, element_offset: usize) -> SymVarRef {
    SymVarRef {
        name: name.clone(),
        element_offset,
    }
}

fn build_member(spec: &MemberSpec) -> PerVarBytecodes {
    let name = member_name(spec.index);
    let tag = spec.index + 1;

    let literals: Vec<f64> = (0..spec.n_literals)
        .map(|i| (tag * 1000 + i) as f64)
        .collect();
    let graphical_functions: Vec<Vec<(f64, f64)>> = (0..spec.n_gf_tables)
        .map(|i| vec![(i as f64, (tag * 10 + i) as f64)])
        .collect();
    let module_decls: Vec<SymbolicModuleDecl> = (0..spec.n_modules)
        .map(|i| SymbolicModuleDecl {
            model_name: Ident::new(&format!("sub{tag}_{i}")),
            input_set: BTreeSet::new(),
            var: vref(&name, i),
        })
        .collect();
    let static_views: Vec<SymbolicStaticView> = (0..spec.n_views)
        .map(|i| SymbolicStaticView {
            // Cycle the three VARIABLE-backed bases (GH #995 added the two
            // snapshot regions): all three name a variable rather than a merged
            // resource, so only `Temp` is renumbered and the interleaved merge
            // must carry each of the others across untouched.
            base: if spec.n_temps > 0 && i % 2 == 0 {
                SymStaticViewBase::Temp((i % spec.n_temps) as u32)
            } else {
                let var = vref(&name, i);
                match i % 3 {
                    0 => SymStaticViewBase::Var(var),
                    1 => SymStaticViewBase::PrevVar(var),
                    _ => SymStaticViewBase::InitialVar(var),
                }
            },
            dims: smallvec::smallvec![(tag * 10 + i) as u16],
            strides: smallvec::smallvec![1],
            offset: i as u32,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
        })
        .collect();
    let temp_sizes: Vec<(u32, usize)> = (0..spec.n_temps).map(|i| (i as u32, i + 1)).collect();
    let dim_lists: Vec<Vec<u16>> = (0..spec.n_elements)
        .map(|i| vec![(tag + i) as u16])
        .collect();

    let mut code: Vec<SymbolicOpcode> = Vec::new();
    for e in 0..spec.n_elements {
        if e < literals.len() {
            code.push(SymbolicOpcode::LoadConstant { id: e as u16 });
        }
        if e < graphical_functions.len() {
            code.push(SymbolicOpcode::Lookup {
                base_gf: e as u8,
                table_count: 1,
                mode: crate::bytecode::LookupMode::Interpolate,
            });
            // The constant-element lookup form belongs in the stream too. This
            // file asserts no GF-run property -- its obligations are 1:1 opcode
            // conservation, element order and temp non-sharing -- but those
            // apply to every opcode, and an opcode the generator never emits is
            // outside all of them.
            code.push(SymbolicOpcode::LookupDirect {
                base_gf: e as u8,
                table_count: 1,
                elem: 0,
                mode: crate::bytecode::LookupMode::Interpolate,
            });
        }
        if e < module_decls.len() {
            code.push(SymbolicOpcode::EvalModule {
                id: e as u16,
                n_inputs: 0,
            });
        }
        if e < static_views.len() {
            code.push(SymbolicOpcode::PushStaticView { view_id: e as u16 });
            code.push(SymbolicOpcode::PopView {});
        }
        if e < dim_lists.len() {
            code.push(SymbolicOpcode::PushVarViewDirect {
                var: SymVarRef::base(name.clone()),
                dim_list_id: e as u16,
            });
            code.push(SymbolicOpcode::PopView {});
        }
        if e < temp_sizes.len() {
            code.push(SymbolicOpcode::VectorSortOrder {
                write_temp_id: e as u8,
            });
            code.push(SymbolicOpcode::LoadTempConst {
                temp_id: e as u8,
                index: 0,
            });
        }
        if spec.with_loop && e == 0 && spec.n_temps > 0 {
            // Wholly inside this element's segment: `BeginIter` and the
            // backward jump that targets inside it both precede the write.
            code.push(SymbolicOpcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: true,
            });
            code.push(SymbolicOpcode::LoadIterViewAt { offset: 0 });
            code.push(SymbolicOpcode::StoreIterElement {});
            code.push(SymbolicOpcode::NextIterOrJump { jump_back: -2 });
            code.push(SymbolicOpcode::EndIter {});
        }
        code.push(SymbolicOpcode::AssignCurr {
            var: vref(&name, e),
        });
    }
    code.push(SymbolicOpcode::Ret);

    PerVarBytecodes {
        symbolic: SymbolicByteCode { literals, code },
        graphical_functions,
        module_decls,
        static_views,
        temp_sizes,
        dim_lists,
    }
}

fn build_members(specs: &[MemberSpec]) -> HashMap<Ident<Canonical>, PerVarBytecodes> {
    specs
        .iter()
        .map(|spec| (member_name(spec.index), build_member(spec)))
        .collect()
}

/// An element order that INTERLEAVES the members: element 0 of every member,
/// then element 1 of every member that has one, and so on. This is the shape
/// that makes temp sharing unsound, so generating it (rather than a
/// member-at-a-time order) is what gives the M5 property its teeth.
fn interleaved_order(specs: &[MemberSpec]) -> Vec<(Ident<Canonical>, usize)> {
    let max_elements = specs.iter().map(|s| s.n_elements).max().unwrap_or(0);
    let mut order = Vec::new();
    for e in 0..max_elements {
        for spec in specs {
            if e < spec.n_elements {
                order.push((member_name(spec.index), e));
            }
        }
    }
    order
}

fn scc_of(order: Vec<(Ident<Canonical>, usize)>) -> ResolvedScc {
    ResolvedScc {
        members: order.iter().map(|(m, _)| m.clone()).collect(),
        element_order: order,
        phase: SccPhase::Dt,
    }
}

/// Every temp slot the combined stream references, paired with the position of
/// the opcode that references it.
///
/// Two channels, and the second is the one that matters here. An opcode can
/// carry a temp id directly, OR a `PushStaticView { view_id }` can push a view
/// whose base is a `Temp` -- how an array-producing builtin's scratch array is
/// read back. `absorb_non_gf` renumbers that base with the same per-fragment
/// `temp_offset` it renumbers opcodes with, so under `Sum` it must advance per
/// member exactly as the opcode ids do.
///
/// Scanning only channel 1 left a real defect green across the whole
/// repository: a merger that shifted a view's `Temp` base by a fixed base rather than the per-fragment offset
/// instead of the per-fragment offset (correct under `Recycle`, wrong under
/// `Sum`) passed lib 5365/0 and `file_io` 637/0. Every member after the first
/// in a resolved recurrence SCC would read the FIRST member's scratch array --
/// two simultaneously-live temps aliased, M5's exact failure mode, arriving
/// through the channel the scan did not look at.
fn temp_uses(code: &[SymbolicOpcode], views: &[SymbolicStaticView]) -> Vec<(usize, u32)> {
    code.iter()
        .enumerate()
        .filter_map(|(pc, op)| {
            let id: u32 = match op {
                SymbolicOpcode::LoadTempConst { temp_id, .. } => *temp_id as u32,
                SymbolicOpcode::BeginIter {
                    write_temp_id,
                    has_write_temp: true,
                } => *write_temp_id as u32,
                SymbolicOpcode::VectorElmMap { write_temp_id, .. }
                | SymbolicOpcode::VectorSortOrder { write_temp_id }
                | SymbolicOpcode::Rank { write_temp_id }
                | SymbolicOpcode::LookupArray { write_temp_id, .. }
                | SymbolicOpcode::AllocateAvailable { write_temp_id }
                | SymbolicOpcode::AllocateByPriority { write_temp_id } => *write_temp_id as u32,
                SymbolicOpcode::PushStaticView { view_id } => {
                    // Loud on an id the view table cannot answer. A silent
                    // `None` here is the exact shape this scan exists to
                    // eliminate -- a temp channel that goes unread -- so an
                    // out-of-range id must not look like "this opcode uses no
                    // temp". (M1's `denote` reports it too; this is the
                    // belt-and-braces inside the scan itself.)
                    let view = views.get(*view_id as usize).unwrap_or_else(|| {
                        panic!(
                            "PushStaticView names view {view_id} but the table holds {}",
                            views.len()
                        )
                    });
                    match &view.base {
                        SymStaticViewBase::Temp(id) => *id,
                        // Variable-backed bases -- `curr` and the two snapshot
                        // regions -- name no temp channel.
                        SymStaticViewBase::Var(_)
                        | SymStaticViewBase::PrevVar(_)
                        | SymStaticViewBase::InitialVar(_) => return None,
                    }
                }
                _ => return None,
            };
            Some((pc, id))
        })
        .collect()
}

/// Which member each opcode of the combined stream came from, read off the
/// per-element write that terminates each segment.
///
/// Derived from `element_order` rather than from the combined stream's own
/// writes, so it does not assume the thing M6 is about to check.
fn owner_per_opcode(
    combined: &PerVarBytecodes,
    order: &[(Ident<Canonical>, usize)],
    members: &HashMap<Ident<Canonical>, PerVarBytecodes>,
) -> Vec<Ident<Canonical>> {
    // Segment lengths, recomputed from each member's own fragment the same way
    // the combiner cuts them: up to and including that element's write, with
    // any tail after the final write joining the last segment.
    let mut owners = Vec::new();
    for (member, elem) in order {
        let frag = &members[member];
        let len = segment_len(frag, member.as_str(), *elem);
        for _ in 0..len {
            owners.push(member.clone());
        }
    }
    // The combined fragment's single terminal `Ret` has no owner.
    debug_assert_eq!(owners.len() + 1, combined.symbolic.code.len());
    owners
}

/// The length of `member`'s segment for element `elem`, computed from the
/// member's OWN fragment.
///
/// This re-derives the segmentation rule `segment_member_by_element` applies,
/// which is the pattern this file's header criticises in the tests it replaces.
/// It is admissible here for a reason that did not hold there, and the
/// difference is worth stating rather than leaving the reader to infer.
///
/// The deleted temp tests used their re-derivation as the ORACLE: the value it
/// produced was the expected answer, so the test could only fail if the two
/// implementations disagreed -- and since that oracle's output was itself
/// asserted equal to a literal, it could not even do that. Here the
/// re-derivation is not the answer to anything. It only attributes merged
/// opcodes to the member that produced them, so the real assertions (no shared
/// temp slot; writes in `element_order`) have owners to talk about.
///
/// And it is self-checking rather than trusted: `owner_per_opcode` ends in
/// `debug_assert_eq!(owners.len() + 1, combined.symbolic.code.len())`, so if
/// this derivation drifts from production's -- for any reason, including
/// production changing -- the totals stop matching and the test panics rather
/// than quietly attributing opcodes to the wrong member.
fn segment_len(frag: &PerVarBytecodes, member: &str, elem: usize) -> usize {
    let code = &frag.symbolic.code;
    let end = if code.last() == Some(&SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    };
    let body = &code[..end];
    let is_write = |op: &SymbolicOpcode| -> Option<usize> {
        match op {
            SymbolicOpcode::AssignCurr { var }
            | SymbolicOpcode::AssignConstCurr { var, .. }
            | SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name.as_str() == member =>
            {
                Some(var.element_offset)
            }
            _ => None,
        }
    };
    let mut start = 0usize;
    let last_write = body.iter().rposition(|op| is_write(op).is_some());
    for (pc, op) in body.iter().enumerate() {
        if let Some(e) = is_write(op) {
            let is_last = Some(pc) == last_write;
            let end = if is_last { body.len() } else { pc + 1 };
            if e == elem {
                return end - start;
            }
            start = pc + 1;
        }
    }
    0
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// M5 for the interleaving emitter, stated as the SAME non-interleaving
    /// condition the sequential property uses: tag every temp use in the
    /// combined stream with the member that produced it, and require that no
    /// merged temp slot is used by a member, then another, then the first
    /// again.
    ///
    /// Under `element_order` the members' segments alternate, so the ONLY way
    /// to satisfy this is for members not to share slots at all -- which is
    /// what `TempStrategy::Sum` gives them. The property is therefore not a
    /// restatement of "use Sum here"; it is the reason Sum is required, and it
    /// would fail immediately if this call site were switched to `Recycle`.
    #[test]
    fn interleaved_members_never_share_a_temp_slot(specs in member_specs()) {
        check_no_shared_temp_slot(&specs, false)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Non-vacuity guard for the property above, aimed squarely at the SECOND
    /// temp channel.
    ///
    /// The base strategy only sometimes draws a member with both temps and
    /// views, and a case with no `Temp`-based static view exercises nothing of
    /// `temp_uses`' view arm. `forced_view_backed_temp_specs` gives every
    /// member both, and `check_no_shared_temp_slot(.., require_view_temps =
    /// true)` asserts a `Temp`-based view is actually present in the combined
    /// output before checking anything.
    ///
    /// This is the guard the missing channel needed: without it, a scan that
    /// silently stopped resolving views would keep passing on whatever the
    /// draw happened to produce.
    #[test]
    fn forced_view_backed_temps_are_never_shared(specs in forced_view_backed_temp_specs()) {
        check_no_shared_temp_slot(&specs, true)?;
    }
}

/// M5 for the interleaving emitter, over both temp channels. Shared by the
/// arbitrary and the forced-view properties.
fn check_no_shared_temp_slot(
    specs: &[MemberSpec],
    require_view_temps: bool,
) -> Result<(), TestCaseError> {
    let members = build_members(specs);
    let order = interleaved_order(specs);
    let scc = scc_of(order.clone());
    let combined = combine_scc_fragment(&scc, &members)
        .map_err(|e| TestCaseError::fail(format!("well-formed members must combine: {e}")))?;

    if require_view_temps {
        // Assert the COUPLING, not two independent counts. "Some view is
        // Temp-based" and "some view is pushed" can both hold while the pushed
        // ones are all `Var`-based and the view arm of `temp_uses` never
        // resolves anything -- the guard would report covered with the channel
        // unreached, which is the exact failure mode this property exists to
        // prevent. So: count the entries `temp_uses` produced that sit at a
        // `PushStaticView` position. Those are, by construction, temps reached
        // THROUGH a view.
        let via_view = temp_uses(&combined.symbolic.code, &combined.static_views)
            .into_iter()
            .filter(|(pc, _)| {
                matches!(
                    combined.symbolic.code[*pc],
                    SymbolicOpcode::PushStaticView { .. }
                )
            })
            .count();
        prop_assert!(
            via_view >= 2,
            "the view arm of `temp_uses` must resolve at least two temps, or the \
             second temp channel is untested; it resolved {}",
            via_view
        );
    }

    let owners = owner_per_opcode(&combined, &order, &members);
    let mut runs: HashMap<u32, Vec<Ident<Canonical>>> = HashMap::new();
    for (pc, temp) in temp_uses(&combined.symbolic.code, &combined.static_views) {
        if pc >= owners.len() {
            continue; // the terminal Ret
        }
        let owner = &owners[pc];
        let seen = runs.entry(temp).or_default();
        if seen.last() != Some(owner) {
            prop_assert!(
                !seen.contains(owner),
                "combined temp slot {} is used by `{}` again after another member \
                     used it -- interleaved members' live ranges overlap, so a shared \
                     slot aliases two live scratch arrays",
                temp,
                owner.as_str()
            );
            seen.push(owner.clone());
        }
    }

    // The stronger consequence for this emitter: no slot has two owners at
    // all. Kept alongside the general condition rather than instead of it,
    // because it is what `Sum` specifically buys and it fails on a
    // different mutation (a `Sum` that forgot to advance).
    for (temp, seen) in &runs {
        prop_assert_eq!(
            seen.len(),
            1,
            "combined temp slot {} has {} distinct member owners",
            temp,
            seen.len()
        );
    }

    Ok(())
}

/// Every member carries temps AND views based on them, so the second temp
/// channel is exercised in every case.
fn forced_view_backed_temp_specs() -> impl Strategy<Value = Vec<MemberSpec>> {
    let member =
        (1usize..3, 1usize..3, 1usize..4).prop_map(|(n_elements, n_temps, n_views)| MemberSpec {
            index: 0,
            n_elements,
            n_literals: 1,
            n_temps,
            n_modules: 0,
            // Deliberately NOT pinned to 1. `build_member` bases view `i` on a
            // temp only when `i % 2 == 0`, so a range here mixes Temp-based and
            // Var-based views -- which is what makes the coupling assertion
            // above load-bearing rather than incidentally true.
            n_views,
            n_gf_tables: 0,
            with_loop: false,
        });
    prop::collection::vec(member, 2..4).prop_map(|mut specs| {
        for (i, spec) in specs.iter_mut().enumerate() {
            spec.index = i;
        }
        specs
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// M6 for the interleaving emitter: the combine is a pure REORDERING.
    ///
    /// Opcode count is conserved (every member's Ret-stripped opcodes appear,
    /// plus one terminal `Ret`); each per-element write appears exactly once
    /// with its ORIGINAL `SymVarRef`, so `resolve_module` still places it where
    /// the acyclic layout would (AC2.3); and the writes come out in exactly
    /// `element_order`.
    #[test]
    fn interleave_conserves_opcodes_and_follows_element_order(specs in member_specs()) {
        let members = build_members(&specs);
        let order = interleaved_order(&specs);
        let scc = scc_of(order.clone());
        let combined = combine_scc_fragment(&scc, &members).map_err(|e| {
            TestCaseError::fail(format!("well-formed members must combine: {e}"))
        })?;

        let want_len: usize = members
            .values()
            .map(|f| {
                let code = &f.symbolic.code;
                if code.last() == Some(&SymbolicOpcode::Ret) {
                    code.len() - 1
                } else {
                    code.len()
                }
            })
            .sum();
        prop_assert_eq!(
            combined.symbolic.code.len(),
            want_len + 1,
            "the interleave must conserve every member opcode and add one Ret"
        );
        prop_assert_eq!(
            combined.symbolic.code.last(),
            Some(&SymbolicOpcode::Ret),
            "exactly one terminal Ret"
        );

        let member_names: HashSet<String> = specs
            .iter()
            .map(|s| member_name(s.index).as_str().to_string())
            .collect();
        let writes: Vec<SymVarRef> = combined
            .symbolic
            .code
            .iter()
            .filter_map(|op| match op {
                SymbolicOpcode::AssignCurr { var }
                | SymbolicOpcode::AssignConstCurr { var, .. }
                | SymbolicOpcode::BinOpAssignCurr { var, .. }
                    if member_names.contains(var.name.as_str()) =>
                {
                    Some(var.clone())
                }
                _ => None,
            })
            .collect();
        let want_writes: Vec<SymVarRef> = order
            .iter()
            .map(|(member, elem)| vref(member, *elem))
            .collect();
        prop_assert_eq!(
            &writes,
            &want_writes,
            "the combined writes must follow element_order, each keeping its \
             original (name, element_offset)"
        );
    }
}
