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
use crate::db::var_fragment::{
    DeclaredName, fragment_overlay, fragment_reads_module, implicit_fragment_overlay,
    implicit_fragment_reads_module,
};
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

/// Which arm of the module-reach rule a fragment takes, read off the
/// production memos the predicate itself is derived from
/// (`lowered_source_variable`, `lowered_implicit_variable`,
/// `parse_source_variable`). `fragment_reads_module` answers only "any arm",
/// so a row that named no arm could pin the wrong one; this is what lets each
/// row say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ModuleReach {
    /// The variable or helper is itself a module instance, so its OWN shape
    /// is the sub-model's layout.
    IsInstance,
    /// A head it resolves through is an explicit `Module` variable.
    SourceHead,
    /// A head it resolves through is a helper the parse expanded a
    /// module-function call into.
    HelperHead,
    /// The parse minted an instance the equation itself does not read (its
    /// output is read by a capture), so the instance is a key of the
    /// fragment's shapes but no head of the equation.
    UnreadInstance,
    /// No module shape anywhere: the fragment is the same under either
    /// overlay.
    None,
}

fn reach_of_heads(db: &SimlinDb, heads: &[(Ident<Canonical>, DeclaredName)]) -> ModuleReach {
    let is_source_module = |declared: &DeclaredName| matches!(declared, DeclaredName::Source(sv) if sv.kind(db) == SourceVariableKind::Module);
    if heads.iter().any(|(_, declared)| is_source_module(declared)) {
        ModuleReach::SourceHead
    } else if heads
        .iter()
        .any(|(_, declared)| matches!(declared, DeclaredName::Helper(meta) if meta.is_module))
    {
        ModuleReach::HelperHead
    } else {
        ModuleReach::None
    }
}

fn source_reach(
    db: &SimlinDb,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
) -> ModuleReach {
    if var.kind(db) == SourceVariableKind::Module {
        return ModuleReach::IsInstance;
    }
    match reach_of_heads(db, &lowered_source_variable(db, var, model, project).heads) {
        ModuleReach::None
            if parse_source_variable(db, var, project)
                .implicit_vars
                .iter()
                .any(crate::capture::ImplicitVar::is_module) =>
        {
            ModuleReach::UnreadInstance
        }
        reach => reach,
    }
}

fn helper_reach(
    db: &SimlinDb,
    model: SourceModel,
    project: SourceProject,
    name: &str,
) -> ModuleReach {
    let meta = model_implicit_var_by_name(db, model, project, name.to_string())
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: a helper of the fixture's main model"));
    if meta.is_module {
        return ModuleReach::IsInstance;
    }
    let lowered = lowered_implicit_variable(db, model, project, name.to_string())
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: lowers"));
    reach_of_heads(db, &lowered.heads)
}

/// Every variable and helper of `project` whose fragment resolves a module
/// instance's shape, by the production predicate, over every model the
/// diagnostics pass walks (the spliced stdlib templates included). Helpers
/// are named `{parent}#{helper}`, the identity the fragment compilers' body
/// log records them under.
fn module_reading_names(db: &SimlinDb, project: SourceProject) -> (Vec<String>, Vec<String>) {
    let mut vars: Vec<String> = Vec::new();
    let mut helpers: Vec<String> = Vec::new();
    for model in project.models(db).values() {
        for (name, var) in model.variables(db) {
            if fragment_reads_module(db, *var, *model, project) {
                vars.push(name.clone());
            }
        }
        for (name, meta) in model_implicit_var_info(db, *model, project) {
            if implicit_fragment_reads_module(db, *model, project, name.clone()) {
                helpers.push(format!("{}#{name}", meta.parent_source_var.ident(db)));
            }
        }
    }
    vars.sort_unstable();
    helpers.sort_unstable();
    (vars, helpers)
}

/// One variable per arm of the module-reach rule: a module instance; a read
/// of an explicit module's output; a read of an implicit instance's output;
/// an instance minted for a `PREVIOUS` argument, whose output the capture
/// reads and the equation does not; and a hoisted argument reading an
/// explicit module's output, which gives the HELPER rows their source-module
/// arm. `population`, `births`, `rate` and a capture of a plain expression
/// reach nothing.
fn module_reach_fixture() -> datamodel::Project {
    let mut project = TestProject::new("main")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.1", None)
        .aux("from_sub", "sub.out", None)
        .aux("smoothed", "SMTH1(population, 3)", None)
        .aux("lagged", "PREVIOUS(SMTH1(rate, 3), 0)", None)
        .aux("hoisted", "SMTH1(sub.out + 1, 3)", None)
        .aux("plain_capture", "PREVIOUS(rate + 1, 0)", None)
        .build_datamodel();
    project.models[0]
        .variables
        .push(crate::testutils::x_module_named("sub", "child", &[], None));
    project.models.push(crate::testutils::x_model(
        "child",
        vec![crate::testutils::x_aux("out", "1", None)],
    ));
    project
}

/// `fragment_reads_module` and its helper twin are true through each arm of
/// the rule and false where none holds, and the overlay a fragment is keyed
/// on follows exactly that. Every variable and every helper of the fixture is
/// a row, so an arm cannot go uncovered by a fixture variable quietly
/// drifting onto another one, and each row asserts the arm it reaches off the
/// same memos the predicate reads.
#[test]
fn a_fragment_reads_a_module_through_each_arm_of_the_rule() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &module_reach_fixture());
    let (model, project) = (sync.models["main"].source, sync.project);

    let explicit_rows = [
        ("births", ModuleReach::None),
        ("from_sub", ModuleReach::SourceHead),
        ("hoisted", ModuleReach::HelperHead),
        ("lagged", ModuleReach::UnreadInstance),
        ("plain_capture", ModuleReach::None),
        ("population", ModuleReach::None),
        ("rate", ModuleReach::None),
        ("smoothed", ModuleReach::HelperHead),
        ("sub", ModuleReach::IsInstance),
    ];
    let mut all_explicit: Vec<&str> = sync.models["main"]
        .variables
        .keys()
        .map(String::as_str)
        .collect();
    all_explicit.sort_unstable();
    assert_eq!(
        all_explicit,
        explicit_rows
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        "every explicit variable of the fixture is a row"
    );
    for (name, reach) in explicit_rows {
        let var = sync.models["main"].variables[name].source;
        assert_eq!(
            source_reach(&db, var, model, project),
            reach,
            "{name}: the fixture reaches the arm its row names"
        );
        let reads_module = fragment_reads_module(&db, var, model, project);
        assert_eq!(
            reads_module,
            reach != ModuleReach::None,
            "{name}: the predicate is exactly 'some arm holds'"
        );
        assert_eq!(
            fragment_overlay(&db, var, model, project, LtmOverlay::On),
            if reads_module {
                LtmOverlay::On
            } else {
                LtmOverlay::Off
            },
            "{name}: keyed on the requested overlay exactly when it reads a module"
        );
        assert_eq!(
            fragment_overlay(&db, var, model, project, LtmOverlay::Off),
            LtmOverlay::Off,
            "{name}: a plain compile always asks for the plain key"
        );
    }

    let helper_rows = [
        ("$⁚hoisted⁚0⁚arg0", ModuleReach::SourceHead),
        ("$⁚hoisted⁚0⁚arg1", ModuleReach::None),
        ("$⁚hoisted⁚0⁚smth1", ModuleReach::IsInstance),
        ("$⁚lagged⁚0⁚arg1", ModuleReach::None),
        ("$⁚lagged⁚0⁚smth1", ModuleReach::IsInstance),
        ("$⁚lagged⁚1⁚arg0", ModuleReach::HelperHead),
        ("$⁚plain_capture⁚0⁚arg0", ModuleReach::None),
        ("$⁚smoothed⁚0⁚arg1", ModuleReach::None),
        ("$⁚smoothed⁚0⁚smth1", ModuleReach::IsInstance),
    ];
    let mut helpers: Vec<(String, ModuleReach, bool)> =
        model_implicit_var_info(&db, model, project)
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    helper_reach(&db, model, project, name),
                    implicit_fragment_reads_module(&db, model, project, name.clone()),
                )
            })
            .collect();
    helpers.sort();
    assert_eq!(
        helpers,
        helper_rows
            .iter()
            .map(|(name, reach)| ((*name).to_string(), *reach, *reach != ModuleReach::None))
            .collect::<Vec<_>>(),
        "every helper of the fixture is a row, reaches the arm its row names, \
         and reads a module exactly when an arm holds"
    );
    for (name, reach) in helper_rows {
        assert_eq!(
            implicit_fragment_overlay(&db, model, project, name.to_string(), LtmOverlay::On),
            if reach == ModuleReach::None {
                LtmOverlay::Off
            } else {
                LtmOverlay::On
            },
            "{name}: keyed on the requested overlay exactly when it reads a module"
        );
    }
    // A name the model synthesizes no helper of gets the plain key, which is
    // the key of the `None` fragment its compiler returns.
    let absent = "$\u{205A}nobody\u{205A}0\u{205A}arg0".to_string();
    assert!(!implicit_fragment_reads_module(
        &db,
        model,
        project,
        absent.clone()
    ));
    assert_eq!(
        implicit_fragment_overlay(&db, model, project, absent, LtmOverlay::On),
        LtmOverlay::Off
    );
}

/// The overlay reaches a fragment through one shape only, a module
/// instance's (the sub-model's layout, which carries its LTM section under
/// `On`), so a fragment memo is keyed on the overlay only where the fragment
/// resolves one. A first pass under the second overlay therefore re-emits
/// exactly the module-reading fragments and no others: none at all on the
/// plain loop; on the loop through a module, `smoothed` -- whose head is the
/// SMOOTH instance -- and the instance helper itself, whose own shape is the
/// sub-model's, while `births` (which reads `smoothed` by its dimensions),
/// `population`, `rate`, the hoisted delay-time argument and every variable
/// of the `stdlib⁚smth1` sub-model reuse the memo the plain pass left.
///
/// Names come from the fragment compilers' own body log and distinct keys
/// from salsa's execution events: a memo's value and address are equal
/// either way, so only an execution event separates "reused" from
/// "recompiled and found equal".
#[test]
fn a_second_overlay_re_emits_only_the_module_reading_fragments() {
    for (fixture, datamodel) in fixtures() {
        let mut probed = ProbedDb::new();
        let project = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None).project;
        let pass = |probed: &ProbedDb, overlay: LtmOverlay| {
            assemble_simulation(probed.db(), project, "main".to_string(), overlay)
                .unwrap_or_else(|e| panic!("{fixture}: assembles under {overlay:?}: {e:?}"));
            collect_all_diagnostics(probed.db(), project, overlay);
        };
        pass(&probed, LtmOverlay::Off);

        let (expected_explicit, expected_implicit): (Vec<&str>, Vec<&str>) =
            if fixture == "plain loop" {
                (vec![], vec![])
            } else {
                (
                    vec!["smoothed"],
                    vec!["smoothed#$\u{205A}smoothed\u{205A}0\u{205A}smth1"],
                )
            };
        // The same expectation, derived by running the predicate over every
        // variable and helper of every model the two passes walk (the stdlib
        // templates the sync splices in included), so what re-emits is
        // measured against the rule and not only against a hand-written list.
        // Demanded BEFORE the measured region so the predicate's own memos
        // are not what the region counts.
        assert_eq!(
            module_reading_names(probed.db(), project),
            (
                expected_explicit
                    .iter()
                    .map(|n| (*n).to_string())
                    .collect::<Vec<_>>(),
                expected_implicit
                    .iter()
                    .map(|n| (*n).to_string())
                    .collect::<Vec<_>>()
            ),
            "{fixture}: the predicate names exactly the fragments expected to re-emit"
        );

        probed.reset();
        reset_fragment_executions();
        pass(&probed, LtmOverlay::On);
        let execs = fragment_executions();
        let of_kind = |kind: FragmentExecKind| -> Vec<&str> {
            execs
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, name)| name.as_str())
                .collect()
        };
        assert_eq!(
            of_kind(FragmentExecKind::Explicit),
            expected_explicit,
            "{fixture}: the second overlay must re-emit the explicit fragments \
             that resolve a module shape and no others"
        );
        assert_eq!(
            of_kind(FragmentExecKind::Implicit),
            expected_implicit,
            "{fixture}: the second overlay must re-emit the helper fragments \
             that resolve a module shape and no others"
        );

        let counts = probed.counts();
        let distinct_keys = |query: &str| counts.get(query).map(|(_, keys)| *keys).unwrap_or(0);
        assert_eq!(
            distinct_keys("compile_var_fragment"),
            expected_explicit.len(),
            "{fixture}: one new explicit fragment memo per module-reading \
             variable; got {counts:?}"
        );
        assert_eq!(
            distinct_keys("compile_implicit_var_fragment"),
            expected_implicit.len(),
            "{fixture}: one new helper fragment memo per module-reading \
             helper; got {counts:?}"
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
