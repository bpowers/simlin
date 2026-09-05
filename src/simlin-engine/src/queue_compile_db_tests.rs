// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the special-stock build path's relationship to the caller's salsa
//! database ([`super::compile_sim`] / [`super::build_compiled`]): expanded-input
//! reuse across edits and across conveyor/ordinary toggles, `<overflow/>` marker
//! validation on every compile path (the VM dispatch AND the wasm backend, which
//! bypasses it), diagnostics provenance, and staged-patch rollback safety.
//!
//! Split out of `queue_compile.rs` (whose `#[cfg(test)] mod tests` covers the
//! expansion/runtime semantics) to keep that file under the per-file line cap;
//! included via `#[path]` so `use super::*` still resolves private items.

use super::*;
use crate::common::{ErrorCode, Ident};
use crate::db::{ModuleInputSet, SimlinDb, collect_all_diagnostics, compile_var_fragment};
use salsa::plumbing::AsId;
use std::io::BufReader;

fn parse(xml: &str) -> datamodel::Project {
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap()
}

/// The steady-state conveyor fixture plus two ordinary auxes: `unrelated` is
/// never touched (its expanded fragment must stay a salsa cache hit) and
/// `edited` is the single variable an incremental edit changes.
fn conveyor_project_with_auxes() -> datamodel::Project {
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile").replace(
        "</variables>",
        r#"<aux name="unrelated"><eqn>7</eqn></aux>
           <aux name="edited"><eqn>1</eqn></aux>
         </variables>"#,
    );
    parse(&xml)
}

/// Rewrite `edited`'s scalar equation in place.
fn set_edited_equation(project: &mut datamodel::Project, eqn: &str) {
    for v in &mut project.models[0].variables {
        if let datamodel::Variable::Aux(a) = v
            && a.ident == "edited"
        {
            a.equation = datamodel::Equation::Scalar(eqn.to_string());
        }
    }
}

/// A model with an `<overflow/>` marker on a flow and NO queue stock anywhere.
///
/// `<overflow/>` rides on a FLOW; both dispatch predicates
/// (`project_has_conveyor`/`project_has_queue`) scan for a marked STOCK. So a model
/// shaped like THIS one -- stray marker, no conveyor or queue stock anywhere -- took
/// the ordinary compile branch, was never validated, and simulated the overflow flow
/// as an ordinary one. `QueueOverflowNotOnQueue` was reachable only for a genuine
/// conveyor/queue model, via `expand_queues`' pre-expansion check (as
/// `overflow_on_a_queues_first_outflow_is_rejected_through_the_dispatch` shows).
///
/// Exercised below through each entry point that compiles it -- `build_sim`,
/// `compile_sim`, and the wasm backend -- all of which bottom out in
/// `compile_project_incremental`, where the check now lives.
const OVERFLOW_WITHOUT_QUEUE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="src"><eqn>100</eqn><outflow>drain</outflow></stock>
    <flow name="drain"><eqn>1</eqn><overflow/></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
  </variables></model>
</xmile>"#;

// ── The `<overflow/>`-on-a-flow dispatch hole ──────────────────────────

/// `build_sim` must reject a stray `<overflow/>` even though its stock-marker
/// scan routes the model down the ordinary compile branch. The three existing
/// `validate_overflow_markers` tests call `expand_queues` directly, a function
/// production never reaches for this model.
#[test]
fn overflow_marker_without_queue_is_rejected_by_build_sim() {
    let project = parse(OVERFLOW_WITHOUT_QUEUE);
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);

    let err = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect_err("a stray <overflow/> must be rejected on the ordinary dispatch branch");
    assert_eq!(err.code, ErrorCode::QueueOverflowNotOnQueue);
    assert!(
        err.details.as_deref().unwrap_or("").contains("drain"),
        "message names the offending flow: {err:?}"
    );
}

/// The same hole through `compile_sim`, the shared dispatch `simlin_sim_new` now
/// funnels through: an overflow-only model must never reach the VM.
#[test]
fn overflow_marker_without_queue_is_rejected_by_compile_sim() {
    let project = parse(OVERFLOW_WITHOUT_QUEUE);
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);

    // `SimBuild` is deliberately not `Debug` (a `CompiledSimulation`'s debug
    // formatting is feature-gated), so unwrap the error half explicitly.
    let err = compile_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .err()
        .expect("stray <overflow/> rejected");
    assert_eq!(err.code, ErrorCode::QueueOverflowNotOnQueue);
}

/// `wasmgen::compile_datamodel_to_artifact` is a production path
/// (`simlin_model_compile_to_wasm`) that never touches `compile_sim`: its own
/// up-front reject scans for special-stock STOCKS, then it calls
/// `compile_project_incremental` directly. Validating markers in the dispatch would
/// have left the wasm backend silently accepting a stray overflow while the VM
/// rejected it -- a backend divergence. Both must reject.
#[test]
fn overflow_marker_without_queue_is_rejected_by_the_wasm_backend() {
    let project = parse(OVERFLOW_WITHOUT_QUEUE);
    let main = project.models[0].name.clone();

    let err = crate::wasmgen::compile_datamodel_to_artifact(&project, &main, false, false)
        .err()
        .expect("the wasm backend must reject a stray <overflow/> too");
    let crate::wasmgen::WasmGenError::Unsupported(msg) = err;
    assert!(
        msg.contains("QueueOverflowNotOnQueue"),
        "the wasm compile error must carry the marker-placement rejection: {msg}"
    );
}

/// A stray `<overflow/>` in a NON-main model. Conveyor/queue support is main-model
/// only, so a sub-model flow can never be a queue outflow: the marker is invalid
/// there by construction. This is a decision, not an omission -- the alternative
/// (silently ignoring sub-model markers) hides a modelling error.
#[test]
fn overflow_marker_in_a_submodel_is_rejected() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <module name="child"/>
    <aux name="a"><eqn>1</eqn></aux>
  </variables></model>
  <model name="child"><variables>
    <stock name="src"><eqn>100</eqn><outflow>drain</outflow></stock>
    <flow name="drain"><eqn>1</eqn><overflow/></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    let err = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect_err("a sub-model <overflow/> must be rejected");
    assert_eq!(err.code, ErrorCode::QueueOverflowNotOnQueue);
    assert!(err.details.as_deref().unwrap_or("").contains("drain"));
}

/// The `<overflow/>` on a queue's FIRST outflow is the one violation the salsa-side
/// validator structurally cannot see: `expand_queues` clears the marker on every
/// driven outflow, so by the time the expanded project reaches
/// `compile_project_incremental` the evidence is gone. That is why the datamodel
/// adapter still runs pre-expansion, and why deleting it would be silent breakage.
#[test]
fn overflow_on_a_queues_first_outflow_is_rejected_through_the_dispatch() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn><overflow/></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    let err = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect_err("an overflow on the first outflow must be rejected");
    assert_eq!(err.code, ErrorCode::QueueOverflowNotOnQueue);
    assert!(
        err.details
            .as_deref()
            .unwrap_or("")
            .contains("first (highest-priority) outflow"),
        "the first-outflow arm must be the one that fires: {err:?}"
    );
}

// ── Incrementality of the expanded compile ─────────────────────────────

/// The core claim: an unrelated single-variable edit on a conveyor model must
/// NOT rebuild the expanded project's salsa inputs, and must NOT recompile the
/// untouched variables' fragments.
///
/// Evidence follows the established `db::fragment_cache_tests` pattern: salsa
/// input identity (`as_id`) proves the inputs were reused rather than recreated,
/// and pointer equality of the `compile_var_fragment` memo reference proves the
/// query was a cache hit. Before the db-resident expanded slot, `build_compiled`
/// synced onto a throwaway `SimlinDb` with a `None` prior state, so BOTH
/// assertions were impossible: every input and every fragment was brand new.
#[test]
fn unrelated_edit_reuses_expanded_inputs_and_fragments() {
    let project = conveyor_project_with_auxes();
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    let mut vm = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect("first conveyor build");
    vm.run_to_end().expect("first run");

    let (expanded_id, model_id, unrelated_id, edited_id, unrelated_frag_ptr, edited_frag1) = {
        let expanded = db
            .expanded_source_project()
            .expect("a conveyor model populates the db's expanded slot");
        let model = expanded.models(&db)["main"];
        let unrelated = model.variables(&db)["unrelated"];
        let edited = model.variables(&db)["edited"];

        let unrelated_frag = compile_var_fragment(
            &db,
            unrelated,
            model,
            expanded,
            ModuleInputSet::empty(&db),
            crate::db::LtmOverlay::Off,
        );
        let edited_frag = compile_var_fragment(
            &db,
            edited,
            model,
            expanded,
            ModuleInputSet::empty(&db),
            crate::db::LtmOverlay::Off,
        );
        assert!(unrelated_frag.is_some() && edited_frag.is_some());

        (
            expanded.as_id(),
            model.as_id(),
            unrelated.as_id(),
            edited.as_id(),
            unrelated_frag as *const _,
            edited_frag.as_ref().unwrap().fragment.clone(),
        )
    };

    // Edit exactly one variable's equation. Nothing else in the project changes.
    let mut project2 = project.clone();
    set_edited_equation(&mut project2, "42");

    let sp2 = db.sync(&project2);
    let mut vm2 = build_sim(&mut db, sp2, &project2, &main, crate::db::LtmOverlay::Off)
        .expect("second conveyor build");
    vm2.run_to_end().expect("second run");

    let expanded2 = db
        .expanded_source_project()
        .expect("expanded slot still populated");
    assert_eq!(
        expanded_id,
        expanded2.as_id(),
        "the expanded SourceProject input must be REUSED across builds, not rebuilt"
    );
    let model2 = expanded2.models(&db)["main"];
    assert_eq!(
        model_id,
        model2.as_id(),
        "the expanded main SourceModel input must be reused"
    );
    let unrelated2 = model2.variables(&db)["unrelated"];
    let edited2 = model2.variables(&db)["edited"];
    assert_eq!(
        unrelated_id,
        unrelated2.as_id(),
        "an untouched expanded variable's input handle must be reused"
    );
    assert_eq!(
        edited_id,
        edited2.as_id(),
        "an edited variable keeps its input handle; only the equation field is set"
    );

    let unrelated_frag2 = compile_var_fragment(
        &db,
        unrelated2,
        model2,
        expanded2,
        ModuleInputSet::empty(&db),
        crate::db::LtmOverlay::Off,
    );
    assert_eq!(
        unrelated_frag_ptr, unrelated_frag2 as *const _,
        "AC: `unrelated`'s expanded fragment must be a salsa cache hit (pointer-equal memo) \
         after an edit to a different variable"
    );

    let edited_frag2 = compile_var_fragment(
        &db,
        edited2,
        model2,
        expanded2,
        ModuleInputSet::empty(&db),
        crate::db::LtmOverlay::Off,
    );
    assert_ne!(
        edited_frag1,
        edited_frag2.as_ref().unwrap().fragment,
        "the edited variable's fragment must actually change (the test would be vacuous otherwise)"
    );

    // And the second build is still correct: the belt is at steady state.
    let students = vm2.get_series(&Ident::new("students")).expect("students");
    for &s in students.iter() {
        assert!((s - 1000.0).abs() < 1e-6, "steady-state students={s}");
    }
    let edited_series = vm2.get_series(&Ident::new("edited")).expect("edited");
    assert!((edited_series[0] - 42.0).abs() < 1e-9);
}

/// Repeated builds must not accumulate expanded inputs: the slot holds exactly
/// one generation of handles, and the expanded main model's variable set stays
/// the same size across edits.
#[test]
fn repeated_builds_do_not_accumulate_expanded_inputs() {
    let project = conveyor_project_with_auxes();
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off).expect("build");

    let (first_id, first_var_count) = {
        let expanded = db.expanded_source_project().expect("expanded slot");
        (
            expanded.as_id(),
            expanded.models(&db)["main"].variables(&db).len(),
        )
    };

    // The cost this slot buys incrementality with: the conveyor model's inputs are
    // in the db TWICE -- once as the user wrote them, once expanded (plus the
    // hidden container/parameter variables the expansion synthesizes).
    let user_var_count = sp.models(&db)["main"].variables(&db).len();
    assert!(
        first_var_count > user_var_count,
        "the expanded model must carry the user's variables plus the synthesized ones \
         ({first_var_count} vs {user_var_count})"
    );

    for i in 0..4 {
        let mut p = project.clone();
        set_edited_equation(&mut p, &format!("{i}"));
        let sp = db.sync(&p);
        build_sim(&mut db, sp, &p, &main, crate::db::LtmOverlay::Off).expect("rebuild");

        let expanded = db.expanded_source_project().expect("expanded slot");
        assert_eq!(
            first_id,
            expanded.as_id(),
            "rebuild {i} must reuse the expanded SourceProject input"
        );
        assert_eq!(
            first_var_count,
            expanded.models(&db)["main"].variables(&db).len(),
            "rebuild {i} must not grow the expanded variable set"
        );
    }
}

/// An ordinary model never allocates the second input slot.
#[test]
fn ordinary_model_has_no_expanded_slot() {
    let project = parse(
        r#"<?xml version="1.0"?><xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
        <header><name>p</name><vendor>t</vendor><product version="1.0">t</product></header>
        <sim_specs method="Euler"><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
        <model><variables><aux name="a"><eqn>1</eqn></aux></variables></model></xmile>"#,
    );
    let main = project.models[0].name.clone();
    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off).expect("ordinary build");
    assert!(
        db.expanded_source_project().is_none(),
        "an ordinary model must not pay for a second SourceProject"
    );
}

/// Strip the conveyor block, turning the belt into an ordinary INTEG. Its outflow
/// was conveyor-driven and so had an empty equation; give it a real one, since an
/// empty equation is legal only on a pass-driven flow.
fn without_conveyor(project: &datamodel::Project) -> datamodel::Project {
    let mut plain = project.clone();
    for v in &mut plain.models[0].variables {
        match v {
            datamodel::Variable::Stock(s) => s.compat.conveyor = None,
            datamodel::Variable::Flow(f) if f.ident == "graduating" => {
                f.equation = datamodel::Equation::Scalar("250".to_string());
            }
            _ => {}
        }
    }
    plain
}

/// Toggling a model between conveyor and ordinary -- which `apply_patch` does on
/// every editor edit, including dry runs and rejected patches -- must not mint a new
/// expanded input set each round trip.
///
/// Salsa never reclaims inputs, so a slot that is *dropped* when the conveyor
/// disappears is not freed; it merely forces the next expanded sync down the
/// `prev == None` path and allocates a SECOND set. The slot is therefore retained
/// across ordinary builds. A stale slot is unobservable -- nothing reads the
/// expanded `SourceProject` without `sync_expanded` re-syncing it first -- which the
/// bit-identical re-simulation below pins.
#[test]
fn conveyor_ordinary_toggles_reuse_one_expanded_input_set() {
    let project = conveyor_project_with_auxes();
    let plain = without_conveyor(&project);
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();

    let sp = db.sync(&project);
    let mut vm = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect("conveyor build");
    vm.run_to_end().expect("run");
    let baseline = vm
        .get_series(&Ident::new("students"))
        .expect("students")
        .to_vec();
    let first_id = db.expanded_source_project().expect("expanded slot").as_id();

    for round in 0..3 {
        // Ordinary build: the slot is retained (stale, unread), not released.
        let sp = db.sync(&plain);
        build_sim(&mut db, sp, &plain, &main, crate::db::LtmOverlay::Off).expect("ordinary build");
        let retained = db
            .expanded_source_project()
            .unwrap_or_else(|| panic!("round {round}: the expanded slot must be retained"));
        assert_eq!(
            first_id,
            retained.as_id(),
            "round {round}: an ordinary build must not drop the expanded input set"
        );

        // Back to the conveyor: re-syncs onto the SAME inputs.
        let sp = db.sync(&project);
        let mut vm = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
            .expect("conveyor rebuild");
        vm.run_to_end().expect("run");
        assert_eq!(
            first_id,
            db.expanded_source_project().expect("expanded slot").as_id(),
            "round {round}: regaining the conveyor must reuse the expanded SourceProject input"
        );

        let after = vm.get_series(&Ident::new("students")).expect("students");
        assert_eq!(baseline.len(), after.len());
        for (i, (a, b)) in baseline.iter().zip(after.iter()).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "round {round} step {i}: a stale expanded slot changed the simulation"
            );
        }
    }
}

// ── Diagnostics provenance (constraint: never from the expanded project) ──

/// `collect_all_diagnostics` runs on the USER's `SourceProject`. Now that the
/// expanded twin lives in the same db, that separation must be verified rather
/// than assumed: a synthetic `$conv$...` ident must never reach a diagnostic,
/// and building the expanded project must not add or remove any diagnostic.
#[test]
fn diagnostics_come_from_the_unexpanded_project() {
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile").replace(
        "</variables>",
        r#"<aux name="broken"><eqn>does_not_exist</eqn></aux></variables>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);

    let before = collect_all_diagnostics(&db, sp, crate::db::LtmOverlay::Off);
    // The expansion succeeds (an unknown reference is a compile-time, not an
    // expansion-time, failure), so the expanded inputs land in the db and the
    // expanded compile then fails.
    build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
        .expect_err("unknown reference fails to compile");
    let after = collect_all_diagnostics(&db, sp, crate::db::LtmOverlay::Off);

    assert_eq!(
        before.len(),
        after.len(),
        "compiling the expanded project must not change the user project's diagnostics"
    );
    assert!(
        !after.is_empty(),
        "the fixture must produce at least one diagnostic, or this test is vacuous"
    );
    assert!(
        after
            .iter()
            .any(|d| d.variable.as_deref() == Some("broken")),
        "the diagnostic must name the user's own variable: {after:?}"
    );

    // The synthetic variables really are in the db -- on the OTHER SourceProject.
    let expanded = db.expanded_source_project().expect("expanded slot");
    let expanded_vars: Vec<String> = expanded.models(&db)["main"]
        .variables(&db)
        .keys()
        .cloned()
        .collect();
    assert!(
        expanded_vars.iter().any(|v| v.starts_with("$conv$")),
        "the expansion must synthesize at least one hidden variable: {expanded_vars:?}"
    );
    for d in &after {
        if let Some(v) = &d.variable {
            assert!(
                !v.starts_with('$'),
                "a synthetic expanded ident leaked into a diagnostic: {v}"
            );
        }
    }
}

// ── Staged-patch rollback safety ───────────────────────────────────────

/// `apply_patch` stages a datamodel, compiles it (expanding it in the process),
/// and rolls back on rejection. The expanded slot must not be left poisoned: the
/// post-rollback build must reproduce the pre-staging simulation exactly.
#[test]
fn rejected_staged_patch_does_not_poison_the_expanded_slot() {
    let project = conveyor_project_with_auxes();
    let main = project.models[0].name.clone();

    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    let baseline = {
        let mut vm = build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off)
            .expect("baseline build");
        vm.run_to_end().expect("baseline run");
        vm.get_series(&Ident::new("students"))
            .expect("students")
            .to_vec()
    };

    // Stage a patch that materially changes the belt's inflow, compile it (this
    // re-syncs the expanded slot to the STAGED project), then roll back.
    let mut staged = project.clone();
    for v in &mut staged.models[0].variables {
        if let datamodel::Variable::Flow(f) = v
            && f.ident == "matriculating"
        {
            f.equation = datamodel::Equation::Scalar("500".to_string());
        }
    }
    let (staged_sp, prev) = db.sync_staged(&staged);
    let mut staged_vm = build_sim(
        &mut db,
        staged_sp,
        &staged,
        &main,
        crate::db::LtmOverlay::Off,
    )
    .expect("staged build");
    staged_vm.run_to_end().expect("staged run");
    let staged_students = staged_vm.get_series(&Ident::new("students")).expect("s");
    assert!(
        (staged_students[staged_students.len() - 1] - baseline[baseline.len() - 1]).abs() > 1.0,
        "the staged patch must actually change the simulation, or this test is vacuous"
    );

    db.restore(&project, prev);

    let sp2 = db.current_source_project().expect("restored");
    let mut vm = build_sim(&mut db, sp2, &project, &main, crate::db::LtmOverlay::Off)
        .expect("post-rollback build");
    vm.run_to_end().expect("post-rollback run");
    let after = vm.get_series(&Ident::new("students")).expect("students");

    assert_eq!(baseline.len(), after.len());
    for (i, (a, b)) in baseline.iter().zip(after.iter()).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "step {i}: post-rollback {b} != baseline {a} (poisoned expanded project)"
        );
    }
}

/// Placing the marker check inside `compile_project_incremental` puts it AFTER the
/// duplicate-variable gate, so a model with both faults reports `DuplicateVariable`
/// -- the precedence the project has always had. (Validating in `compile_sim`, which
/// runs before `compile_project_incremental`, would have inverted it.) For an
/// ordinary model like this fixture the overflow error was previously unreachable, so
/// nothing here is a behavior change; this pins that it stays that way.
#[test]
fn duplicate_variable_outranks_a_stray_overflow_marker() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="src"><eqn>100</eqn><outflow>drain</outflow></stock>
    <flow name="drain"><eqn>1</eqn><overflow/></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
    <aux name="dup a"><eqn>1</eqn></aux>
    <aux name="dup_a"><eqn>2</eqn></aux>
  </variables></model>
</xmile>"#,
    );
    let main = project.models[0].name.clone();
    let mut db = SimlinDb::default();
    let sp = db.sync(&project);
    let err =
        build_sim(&mut db, sp, &project, &main, crate::db::LtmOverlay::Off).expect_err("rejected");
    assert_eq!(err.code, ErrorCode::DuplicateVariable);
}
