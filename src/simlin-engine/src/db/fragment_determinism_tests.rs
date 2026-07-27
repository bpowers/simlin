// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compilation must be a function of its inputs.
//!
//! Every test here compiles the SAME model on independent, freshly-built
//! databases and asserts the result is byte-identical. That is not a style
//! preference: a compiled fragment is a salsa-cached value with a *derived*
//! `PartialEq`, so a field whose order comes from `HashMap` iteration makes two
//! identical compiles compare unequal. Salsa then stops backdating (every
//! downstream consumer re-executes) and the compiled artifact stops being
//! reproducible run to run. Neither symptom shows up in a numeric result, which
//! is how every defect pinned here survived.
//!
//! Repetition rather than a single comparison, because these are probabilistic
//! detectors: Rust's `RandomState` draws a fresh key per `HashMap`, so a
//! two-entry map comes out in the "right" order about half the time. `REPEATS`
//! runs make a miss `2^-(REPEATS-1)`.
//!
//! Deliberately independent of `db::fragment_char_tests`: these pin the
//! determinism *fixes*, the characterization suite pins *behavior*, and the two
//! must be reviewable (and revertable) apart from each other.

use crate::compiler::symbolic::PerVarBytecodes;
use crate::datamodel;
use crate::db::{
    ModuleInputSet, SimlinDb, assemble_simulation, compile_var_fragment, sync_from_datamodel,
};
use crate::test_common::TestProject;

/// Independent compiles per assertion. Twelve makes a missed ordering defect a
/// 1-in-2048 event while costing a few milliseconds.
const REPEATS: usize = 12;

/// Compile one variable's flow-phase fragment on a brand-new database.
fn compile_flow_fragment(dm: &datamodel::Project, var: &str) -> PerVarBytecodes {
    let db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, dm).project;
    let model = *source_project
        .models(&db)
        .get("main")
        .expect("fixture must have a `main` model");
    let source_var = source_project.models(&db)["main"].variables(&db)[var];
    compile_var_fragment(
        &db,
        source_var,
        model,
        source_project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap_or_else(|| panic!("`{var}` must compile"))
    .fragment
    .flow_bytecodes
    .clone()
    .unwrap_or_else(|| panic!("`{var}` must have a flow fragment"))
}

#[track_caller]
fn assert_fragment_stable(dm: &datamodel::Project, var: &str, what: &str) {
    let first = compile_flow_fragment(dm, var);
    for i in 1..REPEATS {
        let again = compile_flow_fragment(dm, var);
        assert_eq!(
            first, again,
            "compile #{i} of `{var}` on a fresh database produced a different \
             fragment; {what} is not a function of the query's inputs, which \
             defeats salsa backdating and makes the compiled artifact \
             irreproducible"
        );
    }
}

/// A fragment carrying more than one temp must be byte-identical every time.
///
/// `temp_sizes` was built straight out of a `HashMap`, so its `(temp_id, size)`
/// vector came out in hash order. `FragmentMerger::absorb` folds those entries
/// order-independently, so no bytecode and no result ever changed -- only the
/// derived `PartialEq` on the cached value. Fixed by
/// `db::assemble::temp_sizes_by_id`.
///
/// The fixture needs TWO array-producing builtins in ONE equation; a single
/// temp cannot express an ordering.
#[test]
fn multi_temp_fragment_is_stable_across_fresh_databases() {
    let dm = TestProject::new("determinism_multi_temp")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .array_with_ranges(
            "arr[region]",
            vec![("east", "30"), ("west", "10"), ("north", "20")],
        )
        .array_aux(
            "combo[region]",
            "VECTOR SORT ORDER(arr[region], 1) + RANK(arr[region], 1)",
        )
        .build_datamodel();

    let probe = compile_flow_fragment(&dm, "combo");
    assert_eq!(
        probe.temp_sizes.len(),
        2,
        "the fixture must put MORE THAN ONE temp in a single fragment, or this \
         test cannot observe an ordering at all"
    );
    assert_fragment_stable(&dm, "combo", "`temp_sizes`");
}

/// A fragment referencing more than one table-bearing variable must be
/// byte-identical every time.
///
/// `Compiler::new` laid out `graphical_functions` -- and therefore every
/// `base_gf` operand its `Lookup`/`LookupArray` opcodes carry -- by iterating
/// `module.tables`, a `HashMap`. Two tables in one fragment meant two different
/// (each self-consistent, each numerically correct) bytecodes across runs. It
/// reaches shipped models: `test/metasd/theil-statistics/Theil_2011.mdl`
/// compiles a fragment holding `["dummy_data", "dummy_simulation"]`.
///
/// Two lookup-only tables plus one consumer reading both is the minimal shape.
#[test]
fn multi_table_fragment_is_stable_across_fresh_databases() {
    let dm = TestProject::new("determinism_multi_table")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux_with_gf("first_table", "", two_point_gf(1.0, 2.0))
        .aux_with_gf("second_table", "", two_point_gf(10.0, 20.0))
        .scalar_aux(
            "consumer",
            "LOOKUP(first_table, time) + LOOKUP(second_table, time)",
        )
        .build_datamodel();

    let probe = compile_flow_fragment(&dm, "consumer");
    assert_eq!(
        probe.graphical_functions.len(),
        2,
        "the fixture must put MORE THAN ONE graphical function in a single \
         fragment, or this test cannot observe an ordering at all"
    );
    assert_fragment_stable(&dm, "consumer", "the `graphical_functions` layout");
}

/// The whole assembled simulation must be byte-identical across fresh
/// databases, not merely each fragment.
///
/// The per-fragment tests above cannot see a defect introduced by assembly
/// itself (fragment merge order, GF dedup, resource renumbering), and the
/// `graphical_functions` ordering defect was originally caught here: the root
/// `CompiledModule` differed on 18 of 23 repeats. `CompiledSimulation` has no
/// `PartialEq`, so this compares the debug rendering of the root module's
/// bytecode and GF table -- coarser than a field-by-field compare, but it is
/// the layer at which the artifact is actually consumed.
#[test]
fn assembled_simulation_is_stable_across_fresh_databases() {
    let dm = TestProject::new("determinism_assembled")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux_with_gf("first_table", "", two_point_gf(1.0, 2.0))
        .aux_with_gf("second_table", "", two_point_gf(10.0, 20.0))
        .scalar_aux(
            "consumer",
            "LOOKUP(first_table, time) + LOOKUP(second_table, time)",
        )
        .named_dimension("region", &["east", "west", "north"])
        .array_with_ranges(
            "arr[region]",
            vec![("east", "30"), ("west", "10"), ("north", "20")],
        )
        .array_aux(
            "combo[region]",
            "VECTOR SORT ORDER(arr[region], 1) + RANK(arr[region], 1)",
        )
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "consumer", None)
        .build_datamodel();

    let render = || {
        let db = SimlinDb::default();
        let source_project = sync_from_datamodel(&db, &dm).project;
        let sim = assemble_simulation(&db, source_project, "main".to_string())
            .expect("fixture must assemble");
        let root = &sim.modules[&sim.root];
        format!(
            "gf={:?}\nflows={:?}\nstocks={:?}",
            root.context.graphical_functions, root.compiled_flows.code, root.compiled_stocks.code
        )
    };

    let first = render();
    for i in 1..REPEATS {
        assert_eq!(
            first,
            render(),
            "assembly #{i} on a fresh database produced a different root \
             CompiledModule; the compiled artifact must be a function of the \
             project alone"
        );
    }
}

/// The LTM emission path has its OWN copy of the compile+symbolize tail, so
/// `temp_sizes_by_id` being called there is a separate fact from it being
/// called in `db::assemble`, and must be pinned separately: reverting either
/// call site alone leaves the other's test green.
///
/// An LTM link score is woven from its TARGET's equation, so a target built
/// from two array-producing builtins yields a synthetic link-score fragment
/// carrying several temps -- six for this fixture.
///
/// This covers `db::ltm::compile::compile_ltm_equation_fragment` (the LTM
/// synthetic-variable tail). The other LTM copy, in
/// `compile_ltm_implicit_var_fragment`, is NOT covered and cannot be: its
/// fragments are the `PREVIOUS` capture auxes the score generators synthesize,
/// which are pinned to a single element (`…⁚arg0⁚east`) and are therefore
/// always scalar. An array-producing builtin needs a whole-array operand, and
/// the one shape that would supply one -- `PREVIOUS` of a wildcard slice --
/// has no `LoadPrev`-of-array-view codegen path and degrades to a warned zero
/// (the GH #517 case `db::ltm_char_tests::char_agg_nested_reducer` pins). So
/// that call site has no reachable temps to order.
#[test]
fn ltm_fragment_with_temps_is_stable_across_fresh_databases() {
    use salsa::Setter;

    let dm = TestProject::new("determinism_ltm_temps")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["east", "west"])
        .array_aux("rate[region]", "0.1")
        .array_flow(
            "growth[region]",
            "(VECTOR SORT ORDER(level[region], 1) + RANK(level[region], 1)) * rate[region] * 0.01",
            None,
        )
        .array_stock("level[region]", "10", &["growth"], &[], None)
        .build_datamodel();

    let compile_score = || {
        let mut db = SimlinDb::default();
        let source_project = sync_from_datamodel(&db, &dm).project;
        source_project.set_ltm_enabled(&mut db).to(true);
        let model = *source_project.models(&db).get("main").unwrap();
        // Reached through the selector `assemble_module` uses, not the
        // `(from, to)`-keyed salsa query: that query re-derives the score as a
        // scalar `Bare` shape, which for an ARRAYED target is the wrong
        // equation and compiles to nothing. `compile_ltm_synthetic_fragment` is
        // the production entry that routes an arrayed score to
        // `compile_ltm_equation_fragment` verbatim.
        let ltm_vars = crate::db::model_ltm_variables(&db, model, source_project);
        let score = ltm_vars
            .vars
            .iter()
            .find(|v| v.name.contains("link_score") && v.name.contains("level\u{2192}growth"))
            .expect("the level->growth link score must be generated");
        crate::db::compile_ltm_synthetic_fragment(&db, score, model, source_project)
            .expect("the level->growth link score must compile")
            .fragment
            .flow_bytecodes
            .clone()
            .expect("the link score must have a flow fragment")
    };

    let first = compile_score();
    assert!(
        first.temp_sizes.len() >= 2,
        "the fixture must give the LTM link-score fragment MORE THAN ONE temp, \
         or this test cannot observe an ordering at all; got {:?}",
        first.temp_sizes
    );
    for i in 1..REPEATS {
        assert_eq!(
            first,
            compile_score(),
            "LTM compile #{i} on a fresh database produced a different \
             link-score fragment"
        );
    }
}

/// A two-point continuous graphical function over x in [0, 1].
fn two_point_gf(y0: f64, y1: f64) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![y0, y1],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale {
            min: y0.min(y1),
            max: y0.max(y1),
        },
    }
}
