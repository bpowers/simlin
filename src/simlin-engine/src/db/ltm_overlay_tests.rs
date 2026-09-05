// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The LTM overlay is an argument of the compile, not an input on the
//! project (`LtmOverlay`): both variants stay memoized side by side, so a
//! pass under one overlay never discards or re-verifies what a pass under the
//! other derived.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::*;
use crate::common::{Canonical, Ident};
use crate::db::exec_probe::ProbedDb;
use crate::test_common::TestProject;

/// The overlay keys two kinds of memo: a model's own layout, assembly and
/// diagnostics, and the shape a cross-module read resolves through
/// (`model_shape` of the sub-model, which carries its LTM section under
/// `On`). A module-free model reaches only the first kind, so the memo
/// tests run over both fixtures: one feedback loop, and the same loop
/// routed through a SMOOTH instance so `births` reads a module output.
fn fixtures() -> [(&'static str, datamodel::Project); 2] {
    let plain = TestProject::new("overlay_keyed")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    let through_module = TestProject::new("overlay_keyed_module")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .aux("smoothed", "SMTH1(population, 3)", None)
        .flow("births", "smoothed * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    [
        ("plain loop", plain),
        ("loop through a module", through_module),
    ]
}

/// How many distinct `model_shape` memos a first pass under both overlays
/// leaves behind. Only a module read resolves through a shape, so the plain
/// fixture leaves none and the module fixture leaves one per overlay for its
/// SMOOTH sub-model. This is the check that the module arm was reached, not
/// merely that nothing re-ran.
fn model_shape_keys(probed: &ProbedDb) -> usize {
    probed
        .counts()
        .get("model_shape")
        .map(|(_, keys)| *keys)
        .unwrap_or(0)
}

/// Every tracked query that ran since the probe's last reset, by name. The
/// one query excluded is `model_all_diagnostics`, whose body re-runs by
/// design on every revision (it reads an untracked value so the accumulator
/// replay stays reachable); it is a walk over memoized children, not work.
fn re_executed(probed: &ProbedDb) -> Vec<String> {
    probed
        .counts()
        .into_keys()
        .filter(|name| name != "model_all_diagnostics")
        .collect()
}

/// Assembling and collecting diagnostics under each overlay in turn leaves
/// the other overlay's memos intact: the second round of the same calls
/// re-executes no tracked query and hands back the same programs.
#[test]
fn both_overlays_stay_memoized_side_by_side() {
    for (fixture, datamodel) in fixtures() {
        let mut probed = ProbedDb::new();
        let project = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None).project;

        let round = |probed: &ProbedDb| {
            let with_overlay =
                assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
                    .unwrap_or_else(|e| panic!("{fixture}: assembles under the overlay: {e:?}"));
            let plain =
                assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
                    .unwrap_or_else(|e| panic!("{fixture}: assembles plain: {e:?}"));
            collect_all_diagnostics(probed.db(), project, LtmOverlay::Off);
            collect_all_diagnostics(probed.db(), project, LtmOverlay::On);
            (with_overlay, plain)
        };

        let (first_on, first_off) = round(&probed);
        assert!(
            first_on.n_slots() > first_off.n_slots(),
            "{fixture}: the overlay's program carries the score slots the plain one does not"
        );
        let expected_shapes = if fixture == "plain loop" { 0 } else { 2 };
        assert_eq!(
            model_shape_keys(&probed),
            expected_shapes,
            "{fixture}: one shape memo per (sub-model, overlay) after the first round"
        );

        probed.reset();
        let (again_on, again_off) = round(&probed);
        assert!(
            Arc::ptr_eq(&first_on, &again_on) && Arc::ptr_eq(&first_off, &again_off),
            "{fixture}: each overlay's assembly must be served from its own memo"
        );
        assert_eq!(
            re_executed(&probed),
            Vec::<String>::new(),
            "{fixture}: interleaving the two overlays must re-execute nothing: each is its own key"
        );
    }
}

/// The overlay-independent derivations are shared: each model's LTM variable
/// set is derived once, whichever overlay first demanded it. The module
/// fixture has two models (main and the SMOOTH sub-model), so two keys.
#[test]
fn the_ltm_derivation_is_derived_once_for_both_overlays() {
    for (fixture, datamodel) in fixtures() {
        let mut probed = ProbedDb::new();
        let sync = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None);
        let (project, model) = (sync.project, sync.models["main"].source_model);

        assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
            .unwrap_or_else(|e| panic!("{fixture}: assembles plain: {e:?}"));
        assert!(
            !probed.counts().contains_key("model_ltm_variables"),
            "{fixture}: a plain assembly must not derive the overlay"
        );

        probed.reset();
        let derived = model_ltm_variables(probed.db(), model, project);
        assert!(!derived.vars.is_empty(), "{fixture}: the loop is scored");
        assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
            .unwrap_or_else(|e| panic!("{fixture}: assembles under the overlay: {e:?}"));
        let models = if fixture == "plain loop" { 1 } else { 2 };
        assert_eq!(
            probed.counts().get("model_ltm_variables").copied(),
            Some((models, models)),
            "{fixture}: the derivation runs once per model and the overlay's assembly reads those memos"
        );
    }
}

/// A resolved recurrence SCC one of whose members reads a module output
/// whose slot moves under the overlay. `m`'s layout puts the instance
/// `inner` -- a sub-model with a feedback loop, so its block carries score
/// slots under `On` -- ahead of `y`, so `ce[t1] = m.y` lowers to a different
/// element offset under each overlay: the one way the two overlays'
/// fragments of an SCC member differ. The `ce`/`ecc` chain is `ref.mdl`'s
/// shape (an element-acyclic recurrence over `t`), and `acc` is initialized
/// from it so the members are scheduled in the initials too.
fn scc_reading_a_module_output() -> datamodel::Project {
    use crate::testutils::{x_arrayed, x_aux, x_flow, x_model, x_module, x_project, x_stock};
    let sim_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 4.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let mut project = x_project(
        sim_specs,
        &[
            x_model(
                "main",
                vec![
                    x_arrayed(
                        "ce",
                        "t",
                        &[("t1", "m.y"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
                    ),
                    x_arrayed(
                        "ecc",
                        "t",
                        &[
                            ("t1", "ce[t1] + 1"),
                            ("t2", "ce[t2] + 1"),
                            ("t3", "ce[t3] + 1"),
                        ],
                    ),
                    x_module("m", &[], None),
                    x_stock("acc", "ecc[t3]", &["inflow"], &[], None),
                    x_flow("inflow", "acc * 0.1", None),
                ],
            ),
            x_model(
                "m",
                vec![
                    x_module("inner", &[], None),
                    x_aux("y", "inner.s + 1", None),
                ],
            ),
            x_model(
                "inner",
                vec![
                    x_stock("s", "10", &["f"], &[], None),
                    x_flow("f", "s * 0.1", None),
                ],
            ),
        ],
    );
    project.dimensions = vec![datamodel::Dimension::named(
        "t".to_string(),
        vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
    )];
    project
}

/// What the recurrence verdict builds a member's element graph from, read
/// off the member's fragment under one overlay through the production
/// pieces the verdict itself uses (`assemble::segment_member_by_element`,
/// `dep_graph::ordering_reads`): the element keys, the prologue's readers,
/// and each slice's current-value reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ElementGraphInputs {
    elements: Vec<usize>,
    prologue_readers: BTreeSet<usize>,
    /// The prologue's reads, then each element's, in `elements` order.
    reads: Vec<BTreeSet<(Ident<Canonical>, usize)>>,
}

impl ElementGraphInputs {
    fn of(
        db: &SimlinDb,
        model: SourceModel,
        project: SourceProject,
        member: &str,
        phase: SccPhase,
        overlay: LtmOverlay,
    ) -> Self {
        let frag = var_phase_symbolic_fragment_prod(db, model, project, member, phase, overlay)
            .unwrap_or_else(|| {
                panic!("{member} {phase:?} {overlay:?}: the fragment is sourceable")
            });
        let seg = super::assemble::segment_member_by_element(
            member,
            &frag.symbolic.code,
            &frag.static_views,
        )
        .unwrap_or_else(|e| panic!("{member} {phase:?} {overlay:?}: segments: {e}"));
        let mut elements: Vec<usize> = seg.segments.keys().copied().collect();
        elements.sort_unstable();
        let reads_of = |ops: &[crate::compiler::symbolic::SymbolicOpcode]| {
            let mut reads = BTreeSet::new();
            for op in ops {
                assert!(
                    super::dep_graph::ordering_reads(op, &frag.static_views, &phase, &mut reads),
                    "{member} {phase:?} {overlay:?}: every read classifies"
                );
            }
            reads
        };
        let mut reads = vec![reads_of(&seg.prologue)];
        reads.extend(elements.iter().map(|e| reads_of(&seg.segments[e])));
        Self {
            elements,
            prologue_readers: seg.prologue_readers,
            reads,
        }
    }

    /// The projection the element graph wires: reads of SCC members only.
    fn of_members(&self, members: &BTreeSet<Ident<Canonical>>) -> Self {
        let reads = self
            .reads
            .iter()
            .map(|slice| {
                slice
                    .iter()
                    .filter(|(name, _)| members.contains(name))
                    .cloned()
                    .collect()
            })
            .collect();
        Self {
            elements: self.elements.clone(),
            prologue_readers: self.prologue_readers.clone(),
            reads,
        }
    }
}

/// The recurrence verdict is taken over the plain overlay's fragments
/// (`dep_graph::symbolic_phase_element_order`) and applied to whichever
/// overlay assembles (`assemble::combine_resolved_sccs`). That holds because
/// the element graph is wired from member writes and members' reads OF
/// MEMBERS, and a module read -- the one thing that differs between the two
/// overlays' fragments of a member -- is never a member. Pinned on the
/// fixture that reaches the arm: the module-reading member's slices read
/// differently under the two overlays (the module read carries a different
/// element offset), while every member's segmentation and member-read
/// projection agree, so the graph, and the order Kahn drains from it, is
/// the same under both; the `On` assembly then consumes the plain-derived
/// order without refusal (the combiner re-segments the `On` fragment and
/// refuses a mismatch loudly).
#[test]
fn a_recurrence_verdict_taken_plain_holds_under_the_overlay() {
    let datamodel = scc_reading_a_module_output();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let (model, project) = (sync.models["main"].source, sync.project);

    let graph = model_dependency_graph(&db, model, project, ModuleInputSet::empty(&db));
    let members: BTreeSet<Ident<Canonical>> = ["ce", "ecc"].into_iter().map(Ident::new).collect();
    // The equations chain ce[t1] -> ecc[t1] -> ce[t2] -> ecc[t2] -> ce[t3] ->
    // ecc[t3]: one element order, whichever overlay's fragments it is read
    // from.
    let element_order: Vec<(Ident<Canonical>, usize)> = (0..3)
        .flat_map(|i| [(Ident::new("ce"), i), (Ident::new("ecc"), i)])
        .collect();
    assert_eq!(
        graph.resolved_sccs,
        vec![ResolvedScc {
            members: members.clone(),
            element_order,
            phase: SccPhase::Dt,
        }],
        "the {{ce, ecc}} recurrence resolves to the chain's element order"
    );
    assert!(!graph.has_cycle(), "{:?}", graph.cycle_variables);

    let mut module_read_moved = false;
    for member in ["ce", "ecc"] {
        for phase in [SccPhase::Dt, SccPhase::Initial] {
            let plain = ElementGraphInputs::of(&db, model, project, member, phase, LtmOverlay::Off);
            let overlaid =
                ElementGraphInputs::of(&db, model, project, member, phase, LtmOverlay::On);
            assert_eq!(
                plain.of_members(&members),
                overlaid.of_members(&members),
                "{member} {phase:?}: the element graph is wired identically under both overlays"
            );
            module_read_moved |= plain != overlaid;
        }
    }
    assert!(
        module_read_moved,
        "the fixture reaches the arm: a member's non-member read differs between the overlays"
    );

    for overlay in [LtmOverlay::Off, LtmOverlay::On] {
        assemble_simulation(&db, project, "main".to_string(), overlay)
            .unwrap_or_else(|e| panic!("assembles under {overlay:?}: {e:?}"));
    }
}
