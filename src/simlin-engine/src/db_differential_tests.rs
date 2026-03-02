// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

/// For every variable in the given model, verify that the phases
/// `compile_var_fragment` produces bytecodes for match the phases
/// the dep graph's runlists include the variable in.
///
/// This is a consistency check: `compile_var_fragment` gates phase
/// compilation on runlist membership. If the dependency extraction
/// refactoring introduced inconsistencies, this check catches them.
///
/// The check is an IMPLICATION: fragment has bytecodes => variable is
/// in the corresponding runlist. The reverse may not hold (a variable
/// can be in a runlist but produce no bytecodes if it has no equation
/// or its compilation yields empty output).
fn assert_fragment_phase_agreement(db: &dyn Db, model: SourceModel, project: SourceProject) {
    let dep_graph = model_dependency_graph(db, model, project);

    for &var in model.variables(db).values() {
        let var_name = var.ident(db);
        let canonical_name = crate::common::canonicalize(var_name).into_owned();
        // Only check root-model variables. Sub-model variables are compiled
        // through the root model path and their phase membership is validated
        // transitively. Extending this to sub-models would require iterating
        // module expansions, which is out of scope for this phase.
        let is_root = true;

        let fragment_result = compile_var_fragment(db, var, model, project, is_root, vec![]);

        // Determine which phases the dep graph includes this variable in
        let in_initials = dep_graph.runlist_initials.contains(&canonical_name);
        let in_flows = dep_graph.runlist_flows.contains(&canonical_name);
        let in_stocks = dep_graph.runlist_stocks.contains(&canonical_name);

        // Determine which phases the fragment produced bytecodes for
        let (frag_initial, frag_flow, frag_stock) = match fragment_result {
            Some(result) => (
                result.fragment.initial_bytecodes.is_some(),
                result.fragment.flow_bytecodes.is_some(),
                result.fragment.stock_bytecodes.is_some(),
            ),
            None => (false, false, false),
        };

        // Fragment compilation gates on runlist membership, so having
        // bytecodes implies being in the runlist. The reverse may not
        // hold (a variable can be in a runlist but produce no bytecodes
        // if it has no equation), so we check the implication direction.
        if frag_initial {
            assert!(
                in_initials,
                "variable '{canonical_name}': fragment has initial bytecodes \
                 but variable is NOT in runlist_initials"
            );
        }
        if frag_flow {
            assert!(
                in_flows,
                "variable '{canonical_name}': fragment has flow bytecodes \
                 but variable is NOT in runlist_flows"
            );
        }
        if frag_stock {
            assert!(
                in_stocks,
                "variable '{canonical_name}': fragment has stock bytecodes \
                 but variable is NOT in runlist_stocks"
            );
        }
    }
}

// ── Task 2: Integration test models ───────────────────────────────────

#[test]
#[cfg(feature = "file_io")]
fn test_fragment_phase_agreement_integration_models() {
    // Representative models from TEST_MODELS covering: simple auxes/flows/stocks,
    // SMOOTH/DELAY stdlib modules, arrayed models, module-backed models, and
    // various expression features.
    let models = &[
        // Simple models (basic stocks, flows, auxes)
        "test/test-models/samples/teacup/teacup.xmile",
        "test/test-models/samples/SIR/SIR.xmile",
        "test/test-models/samples/SIR/SIR_reciprocal-dt.xmile",
        // Module-backed models (stdlib expansion)
        "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
        // SMOOTH/DELAY/TREND stdlib modules
        "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
        "test/test-models/tests/delays2/delays.xmile",
        "test/test-models/tests/trend/test_trend.xmile",
        // Array models (1D, 2D, 3D, A2A, non-A2A)
        "test/test-models/samples/arrays/a2a/a2a.stmx",
        "test/test-models/samples/arrays/non-a2a/non-a2a.stmx",
        "test/test-models/tests/subscript_1d_arrays/test_subscript_1d_arrays.xmile",
        "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
        "test/test-models/tests/subscript_3d_arrays/test_subscript_3d_arrays.xmile",
        "test/test-models/tests/subscript_docs/subscript_docs.xmile",
        "test/test-models/tests/subscript_multiples/test_multiple_subscripts.xmile",
        // Dependency ordering and initialization
        "test/test-models/tests/eval_order/eval_order.xmile",
        "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
        // Misc expression features (lookups, game, inputs)
        "test/test-models/tests/lookups_inline/test_lookups_inline.xmile",
        "test/test-models/tests/game/test_game.xmile",
        "test/test-models/tests/input_functions/test_inputs.xmile",
    ];

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    for path in models {
        let file_path = repo_root.join(path);
        if !file_path.exists() {
            continue;
        }
        let f = std::fs::File::open(&file_path).unwrap();
        let mut f = std::io::BufReader::new(f);
        let datamodel_project = crate::xmile::project_from_reader(&mut f).unwrap();

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel_project);

        // Run agreement check for every model (root and sub-models)
        for model_info in sync.models.values() {
            assert_fragment_phase_agreement(&db, model_info.source, sync.project);
        }
        checked += 1;
    }

    assert!(
        checked > 0,
        "no integration test models found -- ensure tests run from the workspace root"
    );
}

// ── Task 3: Synthetic edge-case models ────────────────────────────────

/// Helper to build a minimal datamodel::Project with a single "main" model.
fn make_project(name: &str, variables: Vec<datamodel::Variable>) -> datamodel::Project {
    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    }
}

fn make_aux(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

#[test]
fn test_fragment_phase_agreement_synthetic_previous_feedback() {
    // x = TIME, y = PREVIOUS(x) + 1
    // y depends on x only through PREVIOUS -- no same-step ordering edge.
    let project = make_project(
        "synth_previous_feedback",
        vec![make_aux("x", "TIME"), make_aux("y", "PREVIOUS(x) + 1")],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    assert_fragment_phase_agreement(&db, model, sync.project);
}

#[test]
fn test_fragment_phase_agreement_synthetic_init_only() {
    // x = TIME, y = INIT(x) + 1
    // y depends on x only through INIT; x must be in initials runlist
    // but is not a dt ordering constraint on y.
    let project = make_project(
        "synth_init_only",
        vec![make_aux("x", "TIME"), make_aux("y", "INIT(x) + 1")],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    assert_fragment_phase_agreement(&db, model, sync.project);
}

#[test]
fn test_fragment_phase_agreement_synthetic_nested_previous() {
    // x = TIME, z = PREVIOUS(PREVIOUS(x))
    // Creates implicit helper variables. All must have consistent phase membership.
    let project = make_project(
        "synth_nested_previous",
        vec![
            make_aux("x", "TIME"),
            make_aux("z", "PREVIOUS(PREVIOUS(x))"),
        ],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    assert_fragment_phase_agreement(&db, model, sync.project);
}

#[test]
fn test_fragment_phase_agreement_synthetic_smooth_module() {
    // x = TIME, y = SMTH1(x, 1)
    // y expands to a stdlib module with internal stocks. The module's
    // implicit variables must all have consistent phases.
    let project = make_project(
        "synth_smooth_module",
        vec![make_aux("x", "TIME"), make_aux("y", "SMTH1(x, 1)")],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    // Check all models (main + stdlib expansion)
    for model_info in sync.models.values() {
        assert_fragment_phase_agreement(&db, model_info.source, sync.project);
    }
}

#[test]
fn test_fragment_phase_agreement_synthetic_mixed_previous_init_current() {
    // x = TIME, y = PREVIOUS(x) + INIT(x) + x
    // x is referenced in all three contexts. The dep graph should have x as a
    // same-step dep of y (because of the bare x reference).
    let project = make_project(
        "synth_mixed_prev_init_current",
        vec![
            make_aux("x", "TIME"),
            make_aux("y", "PREVIOUS(x) + INIT(x) + x"),
        ],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    assert_fragment_phase_agreement(&db, model, sync.project);
}
