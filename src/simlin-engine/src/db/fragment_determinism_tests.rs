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
    ModuleInputSet, SimlinDb, assemble_simulation, collect_all_diagnostics,
    compile_implicit_var_fragment, compile_project_incremental, compile_var_fragment,
    model_dependency_graph, model_implicit_var_info, model_module_ident_context,
    parse_source_variable_with_module_context, sync_from_datamodel,
};
use crate::test_common::TestProject;
use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

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

// ── Implicit-helper identity (GH #1002) ─────────────────────────────────
//
// `BuiltinVisitor` accumulated its synthesized SMOOTH/DELAY/TREND/PREVIOUS
// helpers in a `HashMap` and every producer of `ParsedVariableResult::
// implicit_vars` emitted them with `.values()`. Rust draws a fresh `RandomState`
// key per map, so two parses of the SAME variable -- in the same process, under
// two different `ModuleIdentContext`s -- yielded the helpers in two different
// ORDERS. `ImplicitVarMeta` identified a helper by its POSITION in that vector,
// so a position recorded against one parse resolved to a different helper
// against the other: an aux where a module was expected, or the reverse. Both
// mis-resolutions fail to lower, so the sub-model silently lost every helper
// fragment and the whole project failed to compile -- on a coin flip.
//
// Two fixes, in three production edits. Which test gates which is MEASURED --
// each edit was reverted ALONE, faithfully (the identity revert restores
// `index_in_parent` and indexes with it, rather than some stronger mutation),
// and the whole suite re-run three times:
//
//   * ORDER (a): `BuiltinVisitor::vars` is an `IndexMap`. Reverting reds
//     `implicit_helper_order_is_stable_across_fresh_databases`,
//     `per_element_implicit_helper_order_is_stable_across_fresh_databases`, and
//     `undimensioned_arrayed_helper_bindings_are_stable_across_fresh_databases`
//     -- every route into `implicit_vars` runs through this map.
//   * ORDER (b): `elements_in_stable_order` at the per-element `Ast::Arrayed`
//     call site. Reverting reds `per_element_…` alone.
//   * ORDER (c): `elements_in_stable_order` at the shared-visitor call site.
//     Reverting reds `undimensioned_arrayed_…` alone.
//   * IDENTITY: `ImplicitVarMeta::name` replaces `index_in_parent`. Reverting
//     reds `an_implicit_helper_declines_when_the_contexts_synthesize_different_sets`
//     ALONE.
//
// That last cell is the one worth reading twice, because it is not what a
// first guess predicts. With the order fix standing, a stable order makes a
// POSITIONAL index resolve correctly whenever the two contexts synthesize the
// same helper SET -- so `a_submodel_with_a_bound_input_compiles_on_every_fresh_database`
// and `an_implicit_helper_resolves_to_its_own_name_under_the_instances_input_set`
// gate the ORDER fix, not the identity one. Only a fixture whose two contexts
// synthesize DIFFERENT sets can tell positional from named identity, and there
// is exactly one such test.
//
// Both fixes are kept because they close different properties, and either
// alone would have stopped the reported crash (verified in both directions).
// Identity makes a helper resolve to itself or to nothing, so no compile
// outcome depends on any ordering. Order makes the two salsa-cached vectors
// carrying it (`ParsedVariableResult::implicit_vars`,
// `VariableDeps::implicit_vars`) equal their own recomputation; without it
// those values still differ run to run, salsa stops backdating, and the
// compiled artifact stops being reproducible -- the GH #595 class, invisible in
// every numeric result and in the compile outcome alike.

/// A sub-model whose body makes a stdlib module call over a BOUND input.
///
/// The bound input is what puts two DIFFERENT parse contexts on the same
/// variable: `model_implicit_var_info` derives helper identity under the
/// no-extra-idents context, while assembly resolves it under the context
/// widened by this instance's module-input names.
fn submodel_with_bound_input_project() -> datamodel::Project {
    let sub = x_model(
        "sub",
        vec![
            x_aux("port", "0", None),
            x_aux("sm", "SMTH1(port, 3)", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("x", "5", None),
            x_module("sub", &[("x", "sub.port")], None),
            x_aux("out", "sub.sm", None),
        ],
    );
    x_project(sim_specs_with_units("month"), &[main, sub])
}

/// The single input set `model_name` is instantiated with, as PRODUCTION
/// enumerates it.
///
/// Reading it out of `enumerate_module_instances` rather than writing
/// `["port"]` is what makes the "must be instantiated WITH a bound input" guard
/// below able to fail: a literal round-tripped through `ModuleInputSet` equals
/// itself no matter what the fixture says.
#[track_caller]
fn instantiation_input_set<'db>(
    db: &'db SimlinDb,
    project: crate::db::SourceProject,
    model_name: &str,
) -> ModuleInputSet<'db> {
    let sets = crate::db::assemble::module_input_sets_for(db, project, "main", model_name);
    assert_eq!(
        sets.len(),
        1,
        "fixture must instantiate `{model_name}` exactly once; got {sets:?}"
    );
    ModuleInputSet::from_canonical_set(db, &sets[0])
}

/// One variable's synthesized implicit helpers, in the order reported, each
/// rendered as `name = <content>`.
///
/// The content half is not decoration. Where each slot gets its OWN visitor the
/// instability shows up in the order of the names; where ONE visitor spans
/// every slot the names are `$⁚v⁚0⁚…`, `$⁚v⁚1⁚…`, … handed out by a
/// monotonically increasing counter, so the name LIST is stable no matter which
/// slot is walked first and only the helper's CONTENT moves between the names.
/// A name-only probe is blind to exactly the call site that has no other test:
/// measured, the shared-visitor mutation survives the whole suite against a
/// name-only probe and is caught 3 of 3 against this one.
///
/// A `Variable::Module` has no equation, so rendering only `get_equation()`
/// would degenerate to the bare name for precisely the helper kind that carries
/// the WIRING -- a mutation that re-bound which module instance reads which
/// argument helper would be invisible. Modules therefore render their
/// `model_name` and references instead.
///
/// Still weaker than comparing the compiled artifact, which differs on 21 of 23
/// repeats under the same mutation. This is the cheaper probe that happens to
/// catch the mutations these tests care about; reach for the artifact if that
/// ever stops being true.
fn implicit_helper_signatures(dm: &datamodel::Project, model_name: &str, var: &str) -> Vec<String> {
    let db = SimlinDb::default();
    let project = sync_from_datamodel(&db, dm).project;
    let model = *project
        .models(&db)
        .get(model_name)
        .unwrap_or_else(|| panic!("fixture must have a `{model_name}` model"));
    let source_var = model.variables(&db)[var];
    let ctx = model_module_ident_context(&db, model, project, vec![]);
    parse_source_variable_with_module_context(&db, source_var, project, ctx)
        .implicit_vars
        .iter()
        .map(|v| match v {
            datamodel::Variable::Module(m) => {
                let refs: Vec<String> = m
                    .references
                    .iter()
                    .map(|r| format!("{}->{}", r.src, r.dst))
                    .collect();
                format!("{} = module {} {:?}", v.get_ident(), m.model_name, refs)
            }
            _ => format!("{} = {:?}", v.get_ident(), v.get_equation()),
        })
        .collect()
}

/// No two helpers of one variable may share a canonical name.
///
/// This began as an assumption, was disproved, and is now enforced -- the
/// sequence is worth keeping because the middle step is the defect.
/// `parse_var_with_module_context` runs `parse_and_lower_eqn` TWICE over one
/// variable, once per phase, and both passes name their helpers from a counter
/// that restarts at zero. For a `Scalar`/`ApplyToAll` equation the initial pass
/// returns nothing unless the variable carries an `ACTIVE INITIAL`; the
/// `Arrayed` arm has no such early-out and re-parses every slot, so each helper
/// of a per-element variable used to appear twice.
///
/// While the repeats were byte-identical that was merely wasteful. When the
/// initial pass reads a DIFFERENT equation -- an `Arrayed` element's own init
/// equation, or `compat.active_initial` -- the two passes mint the SAME name
/// for different bodies, `model_implicit_var_info` (name-keyed, last-wins)
/// keeps one, and `compute_layout` gives it one slot. The other body is
/// discarded in silence and one phase runs the other phase's helper. That is
/// pinned as a loud failure by
/// [`an_active_initial_that_collides_a_helper_name_is_refused`].
///
/// `variable::parse_var_with_module_context` now MERGES across the phases
/// instead of appending, so a byte-identical repeat collapses and a genuine
/// collision is an error. Uniqueness is therefore a property of the parse, and
/// this test is what says so for every route into `implicit_vars`.
#[test]
fn implicit_helper_names_are_unique_within_one_parse() {
    let scalar = TestProject::new("uniqueness_scalar")
        .with_sim_time(0.0, 1.0, 1.0)
        .scalar_aux("driver", "5")
        .scalar_aux("combo", "SMTH1(driver, 3) + DELAY1(driver, 2)")
        .build_datamodel();
    let per_element = TestProject::new("uniqueness_per_element")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .scalar_aux("driver", "5")
        .array_with_ranges(
            "arr[region]",
            vec![
                ("east", "SMTH1(driver, 3)"),
                ("west", "SMTH1(driver, 4)"),
                ("north", "SMTH1(driver, 5)"),
            ],
        )
        .build_datamodel();
    let a2a = TestProject::new("uniqueness_a2a")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .array_aux("src[region]", "5")
        .array_aux("arr[region]", "SMTH1(src[region], 3)")
        .build_datamodel();

    for (dm, model, var) in [
        (&scalar, "main", "combo"),
        (&per_element, "main", "arr"),
        (&a2a, "main", "arr"),
        (&undimensioned_arrayed_project(), "main", "arr"),
        (&submodel_with_bound_input_project(), "sub", "sm"),
    ] {
        let sigs = implicit_helper_signatures(dm, model, var);
        assert!(
            !sigs.is_empty(),
            "`{var}` must synthesize helpers, or this row proves nothing"
        );
        let names: Vec<&str> = sigs
            .iter()
            .map(|s| s.split(" = ").next().unwrap())
            .collect();
        let unique: std::collections::BTreeSet<&&str> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "`{var}` reported two helpers under one name: {names:?}. \
             `model_implicit_var_info` is name-keyed and `compute_layout` gives \
             one slot per name, so a repeat is either wasted work or a silently \
             discarded helper body"
        );
    }
}

/// A helper name is not fully context-stable, and the case where it is not is
/// confined to a project that already fails to compile.
///
/// `ImplicitVarMeta::name` replaced a position with a name, and the PR
/// originally claimed that made resolution "this helper or nothing". It does
/// not: synthesized names embed `BuiltinVisitor`'s walk counter, so a context
/// that inserts an EARLIER helper renames every later one. Here the
/// no-extra-idents parse calls `$⁚sm⁚0⁚arg0` the SMTH argument and the widened
/// parse calls it the PREVIOUS capture, so `find_in` hands back a helper the
/// metadata did not mean.
///
/// This test exists to keep that honest and to bound it. The bound is the
/// second half: a collision of this kind REQUIRES the two parses to synthesize
/// different helper sequences, which is exactly what makes the model's layout
/// disagree with its runlists -- so the project fails to compile and no
/// mis-resolved fragment ever runs. If the compile ever starts succeeding here,
/// the mis-resolution stops being harmless and this test is what says so.
///
/// The real fix is context-stable names, which is GH #372's territory.
#[test]
fn a_cross_context_helper_name_collision_is_confined_to_a_failing_compile() {
    let sub = x_model(
        "sub",
        vec![
            x_aux("port", "0", None),
            x_aux("sm", "PREVIOUS(port, 0) + SMTH1(port + 1, 3)", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("x", "5", None),
            x_module("sub", &[("x", "sub.port")], None),
            x_aux("out", "sub.sm", None),
        ],
    );
    let dm = x_project(sim_specs_with_units("month"), &[main, sub]);

    let db = SimlinDb::default();
    let project = sync_from_datamodel(&db, &dm).project;
    let sub_model = *project.models(&db).get("sub").expect("fixture has `sub`");
    let source_var = sub_model.variables(&db)["sm"];

    let helpers = |extra: Vec<String>| -> Vec<(String, String)> {
        let ctx = model_module_ident_context(&db, sub_model, project, extra);
        parse_source_variable_with_module_context(&db, source_var, project, ctx)
            .implicit_vars
            .iter()
            .map(|v| (v.get_ident().to_string(), format!("{:?}", v.get_equation())))
            .collect()
    };
    let derived = helpers(vec![]);
    let resolved = helpers(
        instantiation_input_set(&db, project, "sub")
            .names(&db)
            .clone(),
    );

    // The collision itself: one name, two different helpers.
    let colliding: Vec<&(String, String)> = derived
        .iter()
        .filter(|(name, body)| resolved.iter().any(|(n2, b2)| n2 == name && b2 != body))
        .collect();
    assert!(
        !colliding.is_empty(),
        "the fixture must actually collide a name across the two contexts, or \
         it is not exercising the residual this test documents.\nderived: \
         {derived:?}\nresolved: {resolved:?}"
    );

    // The bound: such a project does not compile, so nothing executes the
    // helper `find_in` mis-identifies.
    let err = compile_project_incremental(&db, project, "main").expect_err(
        "a project whose two parse contexts synthesize different helper \
         sequences must fail to compile -- that failure is what keeps the \
         name collision above harmless",
    );
    assert!(
        err.to_string().contains("failed to compile fragments"),
        "expected the layout-vs-runlist mismatch, got {err}"
    );
}

/// Two phases must not be able to claim one helper name with different bodies.
///
/// The regression test for the cross-phase merge in
/// `variable::parse_var_with_module_context`. `v`'s dt equation and its
/// `ACTIVE INITIAL` both call `SMTH1`, so both passes mint `$⁚v⁚0⁚arg0` -- with
/// `driver * 2` in one and `driver * 100` in the other. Before the merge this
/// project COMPILED, keeping only the initial pass's helper, so the dt phase
/// silently smoothed the wrong expression. Now it is refused.
///
/// Two spellings, and only ONE of them gates the fix -- which is why both are
/// here and why the doc says which is which.
///
/// `same_dep` has both equations reference `driver`, so the surviving helper's
/// dependency set covers the discarded one and nothing downstream objects:
/// before the merge this COMPILED and silently smoothed the wrong expression.
/// That is the arm that reds without the fix.
///
/// `disjoint_dep` references a different variable in each equation. It fails
/// before the fix too, for an unrelated reason -- the discarded helper's
/// dependency is missing from the survivor's dep set, so assembly cannot
/// resolve it. Keeping it makes the pair say something the single fixture
/// cannot: the refusal is not sensitive to whether the two bodies happen to
/// share dependencies. If only this arm existed the test would pass against
/// the very defect it names.
///
/// Refusal, not repair: making this shape WORK needs a phase discriminator in
/// the synthesized helper name, which changes every implicit helper's identity
/// and is its own change. A loud error is the same choice `dedup_vars_by_ident`
/// already makes for the within-one-pass twin of this collision.
#[test]
fn an_active_initial_that_collides_a_helper_name_is_refused() {
    for (label, dt_eqn, init_eqn) in [
        ("same_dep", "SMTH1(driver * 2, 3)", "SMTH1(driver * 100, 7)"),
        ("disjoint_dep", "SMTH1(driver, 1)", "SMTH1(other, 2)"),
    ] {
        let mut dm = TestProject::new("active_initial_helper_collision")
            .with_sim_time(0.0, 2.0, 1.0)
            .scalar_aux("driver", "5")
            .scalar_aux("other", "9")
            .scalar_aux("v", dt_eqn)
            .build_datamodel();
        for var in dm.models[0].variables.iter_mut() {
            if var.get_ident() == "v"
                && let datamodel::Variable::Aux(a) = var
            {
                a.compat.active_initial = Some(init_eqn.to_string());
            }
        }

        let db = SimlinDb::default();
        let project = sync_from_datamodel(&db, &dm).project;
        let err = compile_project_incremental(&db, project, "main")
            .err()
            .unwrap_or_else(|| {
                panic!(
                    "[{label}] a variable whose two phases mint the same helper name \
                 for different bodies must be refused, not compiled with one of \
                 them silently dropped"
                )
            });
        assert!(
            err.to_string().contains('v'),
            "[{label}] the refusal must name the offending variable; got {err}"
        );
    }
}

/// A variable's synthesized implicit helpers must be reported in an order that
/// is a function of its equation.
///
/// This is the root cause of GH #1002. The fixture needs SEVERAL helpers in one
/// variable, because the detector is probabilistic in how many orderings the
/// two maps can disagree over: this one synthesizes FOUR (a temp-arg and a
/// module instance for each of `SMTH1` and `DELAY1`), and `REPEATS`
/// comparisons of four-entry maps make a miss vanishingly unlikely. Two would
/// agree half the time, which is the regime the module header describes.
#[test]
fn implicit_helper_order_is_stable_across_fresh_databases() {
    let dm = TestProject::new("determinism_implicit_order")
        .with_sim_time(0.0, 1.0, 1.0)
        .scalar_aux("driver", "5")
        .scalar_aux("combo", "SMTH1(driver, 3) + DELAY1(driver, 2)")
        .build_datamodel();

    let first = implicit_helper_signatures(&dm, "main", "combo");
    assert!(
        first.len() >= 4,
        "the fixture must synthesize SEVERAL helpers for one variable, or this \
         test cannot observe an ordering at all; got {first:?}"
    );
    for i in 1..REPEATS {
        assert_eq!(
            first,
            implicit_helper_signatures(&dm, "main", "combo"),
            "parse #{i} on a fresh database reported `combo`'s implicit helpers \
             in a different order; helper identity is positional, so an unstable \
             order makes a helper resolve to a different variable depending on \
             which parse asked (GH #1002)"
        );
    }
}

/// A helper's identity must not depend on WHICH parse context resolves it.
///
/// `model_implicit_var_info` keys helpers by canonical name under the
/// no-extra-idents context; `compile_implicit_var_fragment` resolves the same
/// helper under the context widened by the instance's module-input names. The
/// fragment it produces must be the helper the key names. When identity was
/// positional this failed outright -- the two parses disagreed about which
/// position held which helper, and every helper failed to lower.
///
/// Measured against the pre-fix defect, ONE sample of this misses 45% of the
/// time -- the two-entry regime the module header describes -- so it runs the
/// whole check `REPEATS` times on independent databases like its siblings. A
/// single-sample probabilistic detector in a file built around repetition is a
/// coin flip dressed as a test.
#[test]
fn an_implicit_helper_resolves_to_its_own_name_under_the_instances_input_set() {
    for _ in 0..REPEATS {
        check_helpers_resolve_to_their_own_names();
    }
}

#[track_caller]
fn check_helpers_resolve_to_their_own_names() {
    let dm = submodel_with_bound_input_project();
    let db = SimlinDb::default();
    let project = sync_from_datamodel(&db, &dm).project;
    let sub = *project
        .models(&db)
        .get("sub")
        .expect("fixture must have a `sub` model");

    // The input set `sub` is actually instantiated with, not the empty one:
    // the empty set is the context helper identity was DERIVED under, so
    // asserting against it could not observe the disagreement. Taken from
    // production's own enumeration rather than spelled here, so that removing
    // the fixture's binding fails the guard below instead of leaving a
    // hand-written `["port"]` quietly describing wiring that no longer exists.
    let inputs = instantiation_input_set(&db, project, "sub");
    assert!(
        !inputs.names(&db).is_empty(),
        "the fixture's sub-model must be instantiated WITH a bound input, or the \
         two parse contexts coincide and this test proves nothing"
    );
    let dep_graph = model_dependency_graph(&db, sub, project, inputs);

    let info = model_implicit_var_info(&db, sub, project);
    assert!(
        info.len() >= 2,
        "the fixture must synthesize more than one helper in `sub`, or a \
         mis-resolution has nowhere to land; got {info:?}"
    );
    for (name, meta) in info.iter() {
        let fragment =
            compile_implicit_var_fragment(&db, meta, sub, project, dep_graph, inputs.names(&db))
                .unwrap_or_else(|| {
                    panic!(
                        "implicit helper `{name}` failed to lower under its own \
                 instance's module-input set (GH #1002)"
                    )
                });
        assert_eq!(
            &fragment.fragment.ident, name,
            "the fragment compiled for helper `{name}` is actually \
             `{}`; helper identity must not depend on which parse context \
             resolves it (GH #1002)",
            fragment.fragment.ident
        );
    }
}

/// The per-element (`Equation::Arrayed`) expansion has its OWN unordered
/// source, and stabilizing the visitor's accumulator does not reach it.
///
/// That path runs a fresh visitor per slot and unions their helpers, walking
/// the slot map -- an `Ast::Arrayed` `HashMap` -- to do it. So the union's order
/// is the slot map's iteration order no matter how each visitor accumulates.
/// Reverting `elements_in_stable_order` at that one call site leaves every
/// other test in this section green.
///
/// Three slots make a single comparison a 1-in-6 miss; `REPEATS` of them
/// unmissable.
#[test]
fn per_element_implicit_helper_order_is_stable_across_fresh_databases() {
    let dm = TestProject::new("determinism_per_element_order")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("region", &["east", "west", "north"])
        .scalar_aux("driver", "5")
        .array_with_ranges(
            "arr[region]",
            vec![
                ("east", "SMTH1(driver, 3)"),
                ("west", "SMTH1(driver, 4)"),
                ("north", "SMTH1(driver, 5)"),
            ],
        )
        .build_datamodel();

    let first = implicit_helper_signatures(&dm, "main", "arr");
    assert!(
        first.len() >= 3,
        "the fixture must synthesize a helper per slot, or this test cannot \
         observe the slot map's order at all; got {first:?}"
    );
    for i in 1..REPEATS {
        assert_eq!(
            first,
            implicit_helper_signatures(&dm, "main", "arr"),
            "parse #{i} on a fresh database unioned `arr`'s per-slot helpers in \
             a different order; the slot map is a `HashMap`, so the union must \
             walk it in a stable order (GH #1002)"
        );
    }
}

/// An arrayed variable with per-element equations and NO declared dimensions,
/// read through the real XMILE reader.
///
/// `<element subscript="...">` children with no `<dimensions>` sibling is a
/// document an ordinary tool can write, and `xmile::variables`' `convert_equation!`
/// maps the missing `<dimensions>` to an empty dimension list (`None => vec![]`).
/// That empty list is what routes the variable to the SHARED-visitor arm of
/// `instantiate_implicit_modules`' `Ast::Arrayed` branch -- the arm where one
/// visitor walks every slot and hands out the `n` counter that NAMES each
/// synthesized helper.
///
/// Built by parsing, not by hand: the shape is only interesting because
/// production mints it, and a hand-built `Equation::Arrayed(vec![], …)` would
/// be an assumption about the reader rather than a reading of it.
fn undimensioned_arrayed_project() -> datamodel::Project {
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header><vendor>simlin</vendor><product version="1.0">simlin</product><name>t</name></header>
    <sim_specs method="euler"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
    <model>
        <variables>
            <aux name="driver"><eqn>5</eqn></aux>
            <aux name="arr">
                <element subscript="east"><eqn>SMTH1(driver, 3)</eqn></element>
                <element subscript="west"><eqn>SMTH1(driver, 4)</eqn></element>
                <element subscript="north"><eqn>SMTH1(driver, 5)</eqn></element>
            </aux>
        </variables>
    </model>
</xmile>"#;
    let project = crate::compat::open_xmile(&mut xmile.as_bytes())
        .expect("the fixture document must parse as XMILE");
    let arr = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "arr")
        .expect("fixture has an `arr` variable");
    match arr.get_equation() {
        Some(datamodel::Equation::Arrayed(dims, _, _, _)) => assert!(
            dims.is_empty(),
            "the reader must give this variable an EMPTY dimension list, or the \
             fixture does not reach the shared-visitor arm; got {dims:?}"
        ),
        other => panic!("expected an Arrayed equation from the reader, got {other:?}"),
    }
    project
}

/// The shared-visitor arm binds each helper NAME to a slot's equation in
/// iteration order, so that order must be a function of the model too.
///
/// This arm is separate from the per-element one above and has its own call to
/// `elements_in_stable_order`; reverting only this one leaves every other test
/// in the suite green -- it was the one production change in this commit with
/// no coverage until this test existed.
///
/// What moves here is the BINDING, not the name list. One visitor spans every
/// slot, so its `n` counter hands out `$⁚arr⁚0⁚…`, `$⁚arr⁚1⁚…`, `$⁚arr⁚2⁚…` in
/// increasing order however the slots are walked -- the names come out
/// identical every time, and only WHICH slot's equation each name carries
/// moves. A name-only probe is blind to it, which is why
/// `implicit_helper_signatures` renders the equation too. (Measured: with the
/// probe comparing names alone, this mutation survived the whole suite; with
/// equations, it is caught 3 of 3.)
#[test]
fn undimensioned_arrayed_helper_bindings_are_stable_across_fresh_databases() {
    let dm = undimensioned_arrayed_project();

    let first = implicit_helper_signatures(&dm, "main", "arr");
    assert!(
        first.len() >= 3,
        "the fixture must synthesize a helper per slot, or this test cannot \
         observe the counter's order at all; got {first:?}"
    );
    for i in 1..REPEATS {
        assert_eq!(
            first,
            implicit_helper_signatures(&dm, "main", "arr"),
            "parse #{i} on a fresh database bound `arr`'s helper names to \
             different slots' equations; one visitor spans every slot, so an \
             unstable slot order re-binds them (GH #1002)"
        );
    }
}

/// A sub-model whose bound input changes WHICH helpers its body synthesizes.
///
/// `port` carries no `access="input"` flag, so `collect_module_idents` does not
/// see it and the no-extra-idents parse treats `PREVIOUS(port, 0)` as a direct
/// scalar read (`LoadPrev`, no helper). Binding it as a module input widens the
/// context, `is_module_backed_ident` then answers yes, and the same call is
/// rewritten through a synthesized capture aux -- one EXTRA helper, ahead of
/// the `SMTH1` ones in walk order.
///
/// That is the shape a stable ORDER cannot rescue: the two contexts do not
/// disagree about the order of one list, they produce different lists.
fn submodel_whose_input_changes_the_helper_set_project() -> datamodel::Project {
    let sub = x_model(
        "sub",
        vec![
            x_aux("port", "0", None),
            x_aux("sm", "PREVIOUS(port, 0) + SMTH1(port, 3)", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("x", "5", None),
            x_module("sub", &[("x", "sub.port")], None),
            x_aux("out", "sub.sm", None),
        ],
    );
    x_project(sim_specs_with_units("month"), &[main, sub])
}

/// When the two parse contexts synthesize DIFFERENT helper sets, resolution
/// must decline -- never hand back a different helper's fragment under this
/// helper's name.
///
/// Stabilizing the order (which is what fixed the observed GH #1002 failures)
/// leaves this shape open: every position at or after the extra helper still
/// names a different variable in the widened parse than it did in the parse
/// identity was derived from, deterministically. Positional identity therefore
/// produced a fragment whose `ident` was `$⁚sm⁚0⁚arg0` filed under the key
/// `$⁚sm⁚0⁚arg1`. A name resolves to its own helper or to nothing.
///
/// **What this test does NOT claim.** It does not claim the fixture compiles.
/// It does not, on this branch or before it: the model's slot layout is derived
/// from the no-extra-idents parse while its runlists come from the widened one,
/// so the sub-model's helpers have no slots under the names assembly asks for,
/// and the project fails with `failed to compile fragments for variables: …`.
/// That failure is deterministic and PRE-EXISTING (verified byte-identical at
/// the parent commit), and it is a different defect: the module-ident parse
/// context is allowed to change a model's helper SET, which is the tension
/// GH #372 tracks and proposes an explicit model-level parse context for. What
/// changes here is only that the mis-resolution is loud instead of silent.
#[test]
fn an_implicit_helper_declines_when_the_contexts_synthesize_different_sets() {
    let dm = submodel_whose_input_changes_the_helper_set_project();
    let db = SimlinDb::default();
    let project = sync_from_datamodel(&db, &dm).project;
    let sub = *project
        .models(&db)
        .get("sub")
        .expect("fixture must have a `sub` model");
    let source_var = sub.variables(&db)["sm"];
    let inputs = instantiation_input_set(&db, project, "sub");
    assert!(
        !inputs.names(&db).is_empty(),
        "the fixture's sub-model must be instantiated WITH a bound input, or the \
         two parse contexts coincide and this test proves nothing"
    );

    // The premise, derived rather than asserted from reading: the widened
    // context really does synthesize a different helper list. Without this the
    // test would silently degenerate into a duplicate of the sibling above.
    let helpers_under = |extra: Vec<String>| -> Vec<String> {
        let ctx = model_module_ident_context(&db, sub, project, extra);
        parse_source_variable_with_module_context(&db, source_var, project, ctx)
            .implicit_vars
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect()
    };
    let derived_under = helpers_under(vec![]);
    let resolved_under = helpers_under(inputs.names(&db).clone());
    assert_ne!(
        derived_under, resolved_under,
        "the fixture must make the two parse contexts synthesize DIFFERENT \
         helper lists, or it does not exercise anything the order fix left open"
    );

    let dep_graph = model_dependency_graph(&db, sub, project, inputs);
    let info = model_implicit_var_info(&db, sub, project);
    assert!(
        !info.is_empty(),
        "the fixture must derive some helpers, or the loop below is vacuous"
    );
    let mut declined = 0usize;
    for (name, meta) in info.iter() {
        // `None` is the correct answer here: this helper is not in the parse
        // the instance compiles under. What must never happen is `Some` with
        // some other helper's ident.
        match compile_implicit_var_fragment(&db, meta, sub, project, dep_graph, inputs.names(&db)) {
            Some(fragment) => assert_eq!(
                &fragment.fragment.ident, name,
                "the fragment compiled for helper `{name}` is actually `{}`; a \
                 helper must resolve to itself or to nothing, never to another \
                 helper (GH #1002)",
                fragment.fragment.ident
            ),
            None => declined += 1,
        }
    }
    // Without this the loop would also pass if every helper resolved, which is
    // not what this fixture does today and not what the assertion above is
    // there to catch. If a later change makes the two contexts agree on the
    // helper SET -- the GH #372 direction -- this reds, and updating it is the
    // deliberate act of recording that the divergence is gone.
    assert_eq!(
        declined,
        info.len(),
        "every helper of this fixture is absent from the parse its instance \
         compiles under, so every one must decline; got {declined} of {}",
        info.len()
    );
}

/// A sub-model that instantiates a stdlib module over a bound input must
/// compile, every time.
///
/// GH #1002's first observable: this project compiled on some process seeds and
/// failed with the unattributed `failed to compile fragments for variables:
/// $⁚sm⁚0⁚arg1, $⁚sm⁚0⁚smth1` on others.
#[test]
fn a_submodel_with_a_bound_input_compiles_on_every_fresh_database() {
    let dm = submodel_with_bound_input_project();
    for i in 0..REPEATS {
        let db = SimlinDb::default();
        let project = sync_from_datamodel(&db, &dm).project;
        compile_project_incremental(&db, project, "main").unwrap_or_else(|err| {
            panic!(
                "compile #{i} on a fresh database failed: {err}; whether a \
                 sub-model with a bound input compiles must not depend on the \
                 process hash seed (GH #1002)"
            )
        });
    }
}

/// A wiring mistake that is only a WARNING must not decide, at random, whether
/// the implicit helpers around it report errors.
///
/// This is GH #1002's second reported observable, and it is a GUARD rather than
/// a reproduction: it passes at the parent commit too (checked, 120 samples).
/// The reason is structural and worth recording, since a future refactor can
/// take it away. `collect_all_diagnostics` probes the helpers with the EMPTY
/// input set -- `db::diagnostic` chooses that deliberately and says why -- and
/// derives them from `model_implicit_var_info`, which uses the empty context
/// too. Both reads therefore hit the SAME parse memo, so no second context
/// exists on that path and no mis-resolution is expressible there. The moment
/// that probe is changed to use each model's real instantiation input sets (the
/// improvement its own comment describes), this test becomes the thing that
/// notices.
///
/// Binding `sub.phantom`, where `phantom` names no variable of `sub`, is
/// diagnosed as `BadModuleInputDst` and widens this instance's module-input
/// set. The project is well-formed enough to compile, so the wiring warning is
/// the only row it may ever produce.
#[test]
fn a_phantom_module_input_does_not_randomize_implicit_helper_diagnostics() {
    let mut dm = submodel_with_bound_input_project();
    let main = dm
        .models
        .iter_mut()
        .find(|m| m.name == "main")
        .expect("fixture has a main model");
    let module = main
        .variables
        .iter_mut()
        .find(|v| matches!(v, datamodel::Variable::Module(_)))
        .expect("fixture has a module variable");
    if let datamodel::Variable::Module(m) = module {
        m.references.push(datamodel::ModuleReference {
            src: "x".to_string(),
            dst: "sub.phantom".to_string(),
        });
    }

    let render = || {
        let db = SimlinDb::default();
        let project = sync_from_datamodel(&db, &dm).project;
        let mut rows: Vec<String> = collect_all_diagnostics(&db, project)
            .iter()
            .map(|d| format!("{d:?}"))
            .collect();
        rows.sort();
        rows
    };

    let first = render();
    assert!(
        first.iter().any(|row| row.contains("BadModuleInputDst")),
        "the fixture must actually trip the phantom-dst warning, or it is not \
         building the shape GH #1002 was found on; got {first:?}"
    );
    assert!(
        !first.iter().any(|row| row.contains("\u{205A}sm\u{205A}")),
        "a phantom module-input binding must not make `sm`'s implicit helpers \
         report compile errors; got {first:?}"
    );
    for i in 1..REPEATS {
        assert_eq!(
            first,
            render(),
            "diagnostic collection #{i} on a fresh database differed; \
             diagnostics must be a function of the project (GH #1002)"
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
