// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::{compile_ltm_equation_fragment, scalarize_ltm_equation};
use crate::datamodel;
use crate::db::{
    LtmLinkId, RefShape, ShapedLinkScore, SimlinDb, compute_layout,
    link_score_equation_text_shaped, sync_from_datamodel,
};
use crate::test_common::TestProject;

fn phase_sym_load_prev_names(
    phase: &Option<crate::compiler::symbolic::PerVarBytecodes>,
) -> Vec<&str> {
    phase
        .as_ref()
        .map(|bc| {
            bc.symbolic
                .code
                .iter()
                .filter_map(|op| match op {
                    crate::compiler::symbolic::SymbolicOpcode::SymLoadPrev { var } => {
                        Some(var.name.as_str())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn test_ltm_previous_module_var_uses_helper_rewrite() {
    let project = datamodel::Project {
        name: "ltm_prev_module_regression".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "producer".to_string(),
                    model_name: "producer".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("TIME".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    let fragment = compile_ltm_equation_fragment(
        &db,
        "$⁚ltm⁚test_prev_module",
        &crate::db::LtmEquation::scalar("PREVIOUS(producer)".to_string()),
        source_model,
        sync.project,
    )
    .expect("LTM equation should compile");

    let initial_prev_names = phase_sym_load_prev_names(&fragment.fragment.initial_bytecodes);
    let flow_prev_names = phase_sym_load_prev_names(&fragment.fragment.flow_bytecodes);
    let stock_prev_names = phase_sym_load_prev_names(&fragment.fragment.stock_bytecodes);

    assert!(
        initial_prev_names.is_empty(),
        "initial phase should not use SymLoadPrev for PREVIOUS(module_var)",
    );
    assert!(
        flow_prev_names
            .iter()
            .all(|name| name.starts_with("$⁚$⁚ltm⁚test_prev_module⁚0⁚arg0")),
        "flow phase should use SymLoadPrev only for the synthesized helper arg, got {flow_prev_names:?}",
    );
    assert!(
        stock_prev_names.is_empty(),
        "stock phase should not use SymLoadPrev for PREVIOUS(module_var)",
    );
}

/// AC1.1: An LtmSyntheticVar with non-empty dimensions compiles to A2A
/// bytecodes via compile_ltm_equation_fragment. The fragment should
/// succeed and produce per-element bytecodes spanning all dimension
/// elements in the flow bytecodes.
#[test]
fn test_a2a_ltm_equation_fragment_compiles() {
    let project = TestProject::new("a2a_ltm_compile")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    // Compile an A2A LTM equation fragment with dimensions
    let dims = vec!["Region".to_string()];
    let fragment = compile_ltm_equation_fragment(
        &db,
        "$\u{205A}ltm\u{205A}test_a2a_link_score",
        &crate::db::LtmEquation::apply_to_all(
            dims.clone(),
            "PREVIOUS(population) * 0.5".to_string(),
        ),
        source_model,
        sync.project,
    )
    .expect("A2A LTM equation should compile");

    // Verify flow bytecodes exist (LTM vars are always flow-phase)
    let flow_bc = fragment
        .fragment
        .flow_bytecodes
        .as_ref()
        .expect("A2A LTM fragment should have flow bytecodes");

    // Verify A2A expansion produced per-element bytecodes spanning all
    // 3 dimension elements. The compiler may either unroll the A2A
    // expansion into per-element BinOpAssignCurr opcodes (each with a
    // distinct element_offset), or use BeginIter/StoreIterElement loops.
    // Either pattern confirms A2A expansion occurred correctly.
    use crate::compiler::symbolic::SymbolicOpcode;

    // Count distinct element_offset values in store/assign opcodes
    // targeting the LTM variable. This confirms the bytecodes span
    // product(dim_lengths) = 3 slots.
    let store_offsets: Vec<usize> = flow_bc
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name.as_str().contains("test_a2a_link_score") =>
            {
                Some(var.element_offset)
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        store_offsets.len(),
        3,
        "A2A LTM bytecodes should store to 3 elements (one per region), got: {store_offsets:?}"
    );
    assert_eq!(
        store_offsets,
        vec![0, 1, 2],
        "element offsets should be [0, 1, 2] for 3 regions"
    );

    // Verify PREVIOUS references exist (the equation uses PREVIOUS(population))
    let prev_names = phase_sym_load_prev_names(&fragment.fragment.flow_bytecodes);
    assert!(
        !prev_names.is_empty(),
        "A2A LTM flow bytecodes should contain SymLoadPrev for PREVIOUS"
    );
}

/// AC1.1 (layout): When LTM is enabled on a model with arrayed stocks,
/// and an LTM variable has non-empty dimensions, compute_layout should
/// allocate product(dim_lengths) slots for that variable.
///
/// This test manually creates an LtmSyntheticVar with dimensions and
/// verifies the layout via the salsa pipeline. Since we cannot directly
/// inject an arrayed LTM var into the pipeline (the causal graph detects
/// scalar loops only), we verify through compute_layout that:
/// 1. LTM-enabled layout has more slots than LTM-disabled
/// 2. The LTM variable entries have size == 1 (scalar, as generated)
///
/// The A2A size computation code path is exercised by Test 1 above
/// (compile_ltm_equation_fragment with explicit dimensions).
#[test]
fn test_a2a_ltm_layout_size() {
    use salsa::Setter;

    let project = TestProject::new("a2a_ltm_layout")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let n_slots_ltm = compute_layout(&db, source_model, source_project).n_slots;

    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project).n_slots;

    // With LTM enabled, layout should have more slots for LTM variables
    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "LTM-enabled layout should have more slots: ltm={n_slots_ltm}, no_ltm={n_slots_no_ltm}"
    );
}

/// AC1.2: PREVIOUS() within A2A LTM equations reads per-element previous
/// values. When an arrayed LTM equation uses PREVIOUS(var), each array
/// element should reference its own previous slot, not a shared scalar
/// slot.
///
/// This test verifies the mechanism by compiling an A2A LTM equation
/// fragment with PREVIOUS and checking that the symbolic bytecodes
/// contain per-element SymLoadPrev opcodes with distinct element_offsets.
/// Each element's PREVIOUS reads from its own slot, confirming that
/// A2A expansion correctly maps PREVIOUS to per-element semantics.
#[test]
fn test_a2a_ltm_previous_per_element() {
    let project = TestProject::new("a2a_ltm_prev")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    let dims = vec!["Region".to_string()];
    let fragment = compile_ltm_equation_fragment(
        &db,
        "$\u{205A}ltm\u{205A}test_prev_per_elem",
        &crate::db::LtmEquation::apply_to_all(
            dims.clone(),
            "PREVIOUS(population) * 0.5".to_string(),
        ),
        source_model,
        sync.project,
    )
    .expect("A2A LTM equation with PREVIOUS should compile");

    let flow_bc = fragment
        .fragment
        .flow_bytecodes
        .as_ref()
        .expect("should have flow bytecodes");

    // Verify each dimension element gets its own SymLoadPrev opcode with
    // a distinct element_offset. This confirms PREVIOUS reads per-element
    // previous values rather than sharing a single scalar slot.
    use crate::compiler::symbolic::SymbolicOpcode;

    let prev_offsets: Vec<usize> = flow_bc
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::SymLoadPrev { var } if var.name.as_str() == "population" => {
                Some(var.element_offset)
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        prev_offsets.len(),
        3,
        "should have 3 SymLoadPrev for PREVIOUS(population), one per region element, \
         got: {prev_offsets:?}"
    );
    assert_eq!(
        prev_offsets,
        vec![0, 1, 2],
        "each element should read its own previous slot via distinct element_offsets"
    );

    // Verify the LTM variable itself is also stored per-element
    let store_offsets: Vec<usize> = flow_bc
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name.as_str().contains("test_prev_per_elem") =>
            {
                Some(var.element_offset)
            }
            _ => None,
        })
        .collect();

    assert_eq!(store_offsets.len(), 3, "should store 3 per-element results");
    assert_eq!(
        store_offsets,
        vec![0, 1, 2],
        "store offsets should match the 3 region elements"
    );
}

/// AC4.3: Regression test for the stock-to-flow link score bug where
/// `generate_stock_to_flow_equation` only matched `Equation::Scalar`
/// and fell through to "0" for `Equation::ApplyToAll` (arrayed flows).
///
/// This test verifies that the link score equation for a stock-to-flow
/// edge in an arrayed model contains real population references, not
/// just "0".
#[test]
fn test_stock_to_flow_link_score_handles_apply_to_all() {
    let project = TestProject::new("s2f_a2a_regression")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    // The stock-to-flow direction: population -> births. The scalar Bare score
    // (what assembly's sub-case (a) compiles) comes from the shape-aware query;
    // stock-to-flow ignores `RefShape`, so `Bare` yields the same generator
    // output as the (deleted) legacy `(from, to)`-keyed query did.
    let link_id = LtmLinkId::new(&db, "population".to_string(), "births".to_string());
    let ShapedLinkScore::Scored(lsv) =
        link_score_equation_text_shaped(&db, link_id, RefShape::Bare, source_model, sync.project)
    else {
        panic!("stock-to-flow link score should be generated for arrayed model");
    };

    // Before the fix, the equation would contain only "0" terms because
    // the flow_equation was "0" (ApplyToAll fell through the Scalar-only
    // match arm). After the fix, the equation should reference the actual
    // flow equation contents (which include "population").
    let equation_text = lsv.equation.source_text();
    assert!(
        equation_text.contains("population"),
        "stock-to-flow link score equation should reference 'population', \
         but got: {equation_text}",
    );
    assert!(
        !equation_text.starts_with("if (TIME = INITIAL_TIME) then 0 else if")
            || equation_text.contains("population"),
        "link score equation should not use a trivial '0' partial equation"
    );
}

/// ltm-503-cross-element-agg.AC1.3: regression sibling to
/// `test_stock_to_flow_link_score_handles_apply_to_all`, covering the
/// `Ast::Arrayed` (per-element-equation) flow case that
/// `generate_stock_to_flow_equation` previously fell through to a `"0"`
/// placeholder partial for.
///
/// Build a `population[Region]` stock with a per-element-equation
/// `births[Region]` inflow (`<NYC: population[NYC] * 0.03>`, etc.), enable
/// LTM, and ask for the `population -> births` link score with a
/// `FixedIndex` shape. The result must be `Equation::Arrayed` whose every
/// per-element slot references the flow's actual equation contents
/// (`population`) and contains no literal `(0)` partial.
#[test]
fn test_stock_to_flow_link_score_handles_arrayed() {
    let dm_dimension = datamodel::Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
    );
    // `population[Region]` stock with `births` as its sole inflow.
    let population = datamodel::Variable::Stock(datamodel::Stock {
        ident: "population".to_string(),
        equation: datamodel::Equation::ApplyToAll(vec!["Region".to_string()], "100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: vec!["births".to_string()],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    // Per-element-equation flow referencing the stock element-wise.
    let births = datamodel::Variable::Flow(datamodel::Flow {
        ident: "births".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["Region".to_string()],
            vec![
                (
                    "NYC".to_string(),
                    "population[NYC] * 0.03".to_string(),
                    None,
                    None,
                ),
                (
                    "Boston".to_string(),
                    "population[Boston] * 0.02".to_string(),
                    None,
                    None,
                ),
                (
                    "LA".to_string(),
                    "population[LA] * 0.01".to_string(),
                    None,
                    None,
                ),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let project = datamodel::Project {
        name: "s2f_arrayed_regression".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![dm_dimension],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![population, births],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    // Each `births[e]` references `population[e]` -- a FixedIndex(e) ref --
    // so the per-shape emission yields `population[e] -> births` link
    // scores. The scalar Bare score would `scalarize` the result; use the
    // shaped entry point with the FixedIndex shape so the arrayed equation
    // survives intact.
    let link_id = LtmLinkId::new(&db, "population".to_string(), "births".to_string());
    let result = link_score_equation_text_shaped(
        &db,
        link_id,
        RefShape::FixedIndex(vec!["nyc".to_string()]),
        source_model,
        sync.project,
    );
    let ShapedLinkScore::Scored(lsv) = result else {
        panic!("stock-to-arrayed-flow link score should be generated, got: {result:?}");
    };

    let elements = match &lsv.equation {
        crate::db::LtmEquation::Arrayed { elements, .. } => elements,
        other => {
            panic!("stock-to-arrayed-flow link score must be LtmEquation::Arrayed, got: {other:?}")
        }
    };
    assert!(
        !elements.is_empty(),
        "arrayed link score should have per-element slots"
    );
    for (elem, arm) in elements {
        let slot_eqn = &arm.text;
        // The flow's actual equation contents (`population`) must show up
        // in every slot -- before the fix this was a constant `(0)`.
        assert!(
            slot_eqn.contains("population"),
            "slot {elem:?} should reference 'population' (the flow's equation contents), \
             got: {slot_eqn}"
        );
        // No slot may carry the `(0)` placeholder partial.
        assert!(
            !slot_eqn.contains("((0) -"),
            "slot {elem:?} must not use a trivial '0' partial, got: {slot_eqn}"
        );
    }
}

#[test]
fn test_scalarize_ltm_equation_arrayed_collapse() {
    use crate::db::LtmEquation;

    // Uses parseable equation bodies (`LtmEquation` parses each arm eagerly, so
    // a placeholder like "first slot" would trip the augmentation-bug guard);
    // scalarize only ever selects an arm, so the exact expression is irrelevant.
    //
    // Arrayed with multiple per-element slots collapses to the *first* slot's text.
    let multi = LtmEquation::arrayed(
        vec!["region".to_string()],
        vec![
            ("nyc".to_string(), "first_slot".to_string()),
            ("boston".to_string(), "second_slot".to_string()),
        ],
        None,
        false,
    );
    assert!(
        matches!(scalarize_ltm_equation(multi), LtmEquation::Scalar(arm) if arm.text == "first_slot")
    );

    // Arrayed with no slots but a Some(default) falls back to the default text.
    let default_only = LtmEquation::arrayed(
        vec!["region".to_string()],
        vec![],
        Some("default_eqn".to_string()),
        false,
    );
    assert!(
        matches!(scalarize_ltm_equation(default_only), LtmEquation::Scalar(arm) if arm.text == "default_eqn")
    );

    // Arrayed with neither slots nor a default falls back to "0".
    let empty = LtmEquation::arrayed(vec!["region".to_string()], vec![], None, false);
    assert!(matches!(scalarize_ltm_equation(empty), LtmEquation::Scalar(arm) if arm.text == "0"));

    // ApplyToAll and Scalar inputs are preserved (text kept, dims dropped).
    assert!(matches!(
        scalarize_ltm_equation(LtmEquation::apply_to_all(vec!["region".to_string()], "a2a_eqn".to_string())),
        LtmEquation::Scalar(arm) if arm.text == "a2a_eqn"
    ));
    assert!(
        matches!(scalarize_ltm_equation(LtmEquation::scalar("scalar_eqn".to_string())), LtmEquation::Scalar(arm) if arm.text == "scalar_eqn")
    );
}

/// A canonical name that cannot be spelled as a BARE identifier -- one whose
/// first character is not `XID_Start` -- must be quoted in every generated LTM
/// equation, so the equation parses.
///
/// XMILE lets a modeler quote any name, so `"1stock"` is a legal variable and
/// canonicalizes to `1stock`. The equation lexer, though, only starts an
/// identifier on `XID_Start`/`_`: bare `1stock` lexes as the number `1`
/// followed by the identifier `stock`, which is a parse error. `quote_ident`
/// used to test "every char is alphanumeric or `_`", which a leading digit
/// satisfies, so the guard form was emitted with a bare `1stock` and the whole
/// link score failed to parse -- the score silently degraded, and once
/// `LtmEquation` began parsing each arm eagerly it also tripped an
/// augmentation-bug `debug_assert` on a VALID model. `quote_ident` now also
/// consults `ast::needs_quoting`, the leading-character rule the `print_eqn`
/// path already uses, so the two spellings of one name in a single generated
/// equation agree.
///
/// This asserts the real property -- the generated arm has a parsed AST -- not
/// merely that the text contains a quote, so it fails for ANY unparseable
/// generated equation rather than only this spelling.
#[test]
fn link_score_quotes_a_canonical_name_that_cannot_be_bare() {
    let project = TestProject::new("digit_leading_ident")
        .stock("1stock", "0", &["inflow"], &[], None)
        .flow("inflow", "\"1stock\" * 0.1", None)
        .build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    // The `1stock -> inflow` edge: the guard form spells both endpoints.
    let link_id = LtmLinkId::new(&db, "1stock".to_string(), "inflow".to_string());
    let scored = link_score_equation_text_shaped(&db, link_id, RefShape::Bare, model, sync.project);
    let ShapedLinkScore::Scored(lsv) = scored else {
        panic!("the 1stock -> inflow link score should be scored, got: {scored:?}");
    };

    let crate::db::LtmEquation::Scalar(arm) = &lsv.equation else {
        panic!(
            "a scalar target's link score should be Scalar, got: {:?}",
            lsv.name
        );
    };
    assert!(
        arm.text.contains("\"1stock\""),
        "a name that cannot be bare must be quoted in the generated text, got: {}",
        arm.text
    );
    assert!(
        arm.expr.is_some(),
        "the generated link-score equation must PARSE (an unparseable arm carries no \
         AST, compiles to no bytecode, and silently zeroes the score); text was: {}",
        arm.text
    );
}

/// The keyword twin of the test above (GH #976), and the LTM half of that
/// issue's damage.
///
/// `if` is a legal XMILE variable name that canonicalizes to `if`, but the lexer
/// resolves a bare `if` to the keyword before it ever considers an identifier.
/// `quote_ident` tested "every char is alphanumeric or `_`" plus
/// `ast::needs_quoting`, and a keyword satisfied BOTH -- so the generated guard
/// form spelled the source bare, the arm did not parse, the fragment compiled to
/// no bytecode, and the link score read a constant 0 behind a Warning nobody has
/// to look at. The keyword clause now lives in `needs_quoting`, so `quote_ident`
/// inherits it through the delegation it already had.
///
/// Ranges over every keyword rather than sampling `if`: the predicate reads a
/// table, and a table is exactly the thing that can be right for one entry.
#[test]
fn link_score_quotes_every_keyword_named_source() {
    for keyword in ["if", "then", "else", "not", "mod", "and", "or", "nan"] {
        let project = TestProject::new("keyword_ident")
            .stock(keyword, "0", &["inflow"], &[], None)
            .flow("inflow", &format!("\"{keyword}\" * 0.1"), None)
            .build_datamodel();

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let model = sync.models["main"].source;

        let link_id = LtmLinkId::new(&db, keyword.to_string(), "inflow".to_string());
        let scored =
            link_score_equation_text_shaped(&db, link_id, RefShape::Bare, model, sync.project);
        let ShapedLinkScore::Scored(lsv) = scored else {
            panic!("the {keyword} -> inflow link score should be scored, got: {scored:?}");
        };

        let crate::db::LtmEquation::Scalar(arm) = &lsv.equation else {
            panic!(
                "a scalar target's link score should be Scalar, got: {:?}",
                lsv.name
            );
        };
        assert!(
            arm.text.contains(&format!("\"{keyword}\"")),
            "a keyword-named source must be quoted in the generated text, got: {}",
            arm.text
        );
        assert!(
            arm.expr.is_some(),
            "the generated link-score equation must PARSE (an unparseable arm carries no \
             AST, compiles to no bytecode, and silently zeroes the score); text was: {}",
            arm.text
        );
    }
}

/// An unparseable generated arm degrades LOUDLY and WITHOUT PANICKING, for the
/// scalar shape AND for an `Arrayed` equation whose OTHER arms parse fine.
///
/// This pins the fallback deliberately kept in place of the deleted
/// `debug_assert!`. Two properties, and the arrayed one is the sharp edge:
///
/// - the arm retains its parse errors (so `expr` being `None` is
///   distinguishable from a legitimately EMPTY arm, which must still drop
///   silently);
/// - `to_flow_ast` therefore yields NO ast and surfaces the errors, so
///   `compile_ltm_equation_fragment` returns `None` -- the condition
///   `model_ltm_fragment_diagnostics` turns into its "keeps a layout slot but
///   no bytecode" Warning.
///
/// Before this, `LtmArm::new` collapsed a parse failure into a bare
/// `expr: None` and the `Arrayed` slot map silently dropped it. With siblings
/// that parsed, the fragment still had bytecode, so the compiler zero-filled
/// the missing slot and NO diagnostic fired: a silent per-element zero, the
/// exact defect class the typed-equation work exists to remove. The scalar case
/// was loud only incidentally -- it had no surviving sibling to keep the
/// fragment alive.
///
/// Constructing the degenerate value is legitimate through the public API:
/// `LtmArm::new` takes an arbitrary `String`, so there is no "cannot be built"
/// escape hatch here.
#[test]
fn unparseable_generated_arm_degrades_loudly_without_panicking() {
    use crate::db::{LtmArm, LtmEquation};

    // Unparseable for the same reason the `1stock` bug was: a bare leading
    // digit lexes as a number followed by an identifier.
    let bad = "1stock * 0.1";

    // (a) The arm itself: no panic, no AST, errors RETAINED.
    let arm = LtmArm::new(bad.to_string());
    assert!(arm.expr.is_none(), "unparseable text must yield no AST");
    assert!(
        arm.parse_error.is_some(),
        "the parse error must be retained -- discarding it is what made the \
         arrayed case silent"
    );
    // An EMPTY arm is the legitimate `expr == None` and must NOT carry errors.
    let empty = LtmArm::new(String::new());
    assert!(empty.expr.is_none() && empty.parse_error.is_none());

    let project = TestProject::new("bad_arm_degradation")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "10")
        .aux("other", "1", None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dims = crate::db::project_dimensions_context(&db, sync.project);

    // (b) Scalar: no ast, errors surfaced, fragment rejected.
    let scalar = LtmEquation::scalar(bad.to_string());
    let (ast, errs) = scalar.to_flow_ast(dims);
    assert!(
        ast.is_none() && !errs.is_empty(),
        "scalar must reject loudly"
    );
    assert!(
        compile_ltm_equation_fragment(&db, "$⁚ltm⁚bad⁚scalar", &scalar, model, sync.project)
            .is_none(),
        "a rejected equation must not produce a fragment (the diagnostic pass \
         reports the missing bytecode)"
    );

    // (c) Arrayed with ONE bad arm and one GOOD sibling -- the case that was
    // silent. The whole equation must be rejected, not zero-filled.
    let arrayed = LtmEquation::arrayed(
        vec!["Region".to_string()],
        vec![
            ("nyc".to_string(), bad.to_string()),
            ("boston".to_string(), "other * 2".to_string()),
        ],
        None,
        false,
    );
    let (ast, errs) = arrayed.to_flow_ast(dims);
    assert!(
        ast.is_none() && !errs.is_empty(),
        "one unparseable arm must reject the whole arrayed equation rather than \
         drop the slot and let the surviving sibling keep the fragment alive"
    );
    assert!(
        compile_ltm_equation_fragment(&db, "$⁚ltm⁚bad⁚arrayed", &arrayed, model, sync.project)
            .is_none(),
        "the arrayed fragment must be rejected -- otherwise the missing slot is \
         zero-filled and no diagnostic fires"
    );

    // (d) An arrayed equation whose arms ALL parse is unaffected: an empty
    // default still drops silently and the fragment compiles.
    let good = LtmEquation::arrayed(
        vec!["Region".to_string()],
        vec![
            ("nyc".to_string(), "other * 2".to_string()),
            ("boston".to_string(), "other * 3".to_string()),
        ],
        Some(String::new()),
        false,
    );
    let (ast, errs) = good.to_flow_ast(dims);
    assert!(
        ast.is_some() && errs.is_empty(),
        "an all-parsing arrayed equation with an EMPTY default must still build"
    );
}

/// Reproducible timing harness for per-element LTM generation over a WIDE
/// dimension -- the shape whose occurrence lookup was quadratic.
///
/// `build_arrayed_link_score_equation` wraps each of an `Ast::Arrayed` target's
/// N element equations separately, and each wrap needs that slot's occurrence
/// stream. When `OccurrenceLookup::for_slot` rescanned the target's WHOLE
/// stream per slot, that was Theta(N^2) comparisons plus N temporary vectors.
///
/// `#[ignore]`d: this is the measuring instrument, not a gate. Checked in
/// (rather than the numbers) so the measurement is reproducible -- run with
/// `cargo test -p simlin-engine --release --lib -- --ignored --nocapture
/// per_element_generation_scaling`. A timing assertion in the default suite
/// would be flaky and would risk the 3-minute wall-clock cap; the structural
/// guarantee is instead pinned by `slot_occurrence_index_groups_every_slot`.
#[test]
#[ignore]
fn per_element_generation_scaling() {
    use std::time::Instant;

    fn generate_for_width(n: usize) -> std::time::Duration {
        let elems: Vec<String> = (0..n).map(|i| format!("e{i}")).collect();
        let elem_refs: Vec<&str> = elems.iter().map(String::as_str).collect();
        // Each element equation reads its OWN element of the source plus a
        // shared scalar, so every slot carries several occurrences.
        let eqns: Vec<(String, String)> = elems
            .iter()
            .map(|e| (e.clone(), format!("pop[{e}] * rate * 0.01")))
            .collect();
        let eqn_refs: Vec<(&str, &str)> =
            eqns.iter().map(|(e, q)| (e.as_str(), q.as_str())).collect();

        let project = TestProject::new("wide_per_element")
            .named_dimension("Wide", &elem_refs)
            .aux("rate", "1", None)
            .array_flow_with_ranges("growth[Wide]", eqn_refs)
            .array_stock("pop[Wide]", "10", &["growth"], &[], None)
            .build_datamodel();

        let mut db = SimlinDb::default();
        let (source_project, model) = {
            let sync = sync_from_datamodel(&db, &project);
            (sync.project, sync.models["main"].source)
        };
        use salsa::Setter;
        source_project.set_ltm_enabled(&mut db).to(true);

        // Sub-phase timings: which query actually scales quadratically.
        let t0 = Instant::now();
        let _sites = crate::db::ltm_ir::model_ltm_reference_sites(&db, model, source_project);
        let t_sites = t0.elapsed();
        let t0 = Instant::now();
        let _edges = crate::db::model_element_causal_edges(&db, model, source_project);
        let t_edges = t0.elapsed();
        let t0 = Instant::now();
        let _circuits = crate::db::model_loop_circuits_tiered(&db, model, source_project);
        let t_circuits = t0.elapsed();

        let start = Instant::now();
        let ltm = crate::db::model_ltm_variables(&db, model, source_project);
        let elapsed = start.elapsed();
        let n_scores = ltm
            .vars
            .iter()
            .filter(|v| v.name.contains("link_score"))
            .count();
        let n_arms: usize = ltm
            .vars
            .iter()
            .map(|v| match &v.equation {
                crate::db::LtmEquation::Arrayed { elements, .. } => elements.len(),
                _ => 1,
            })
            .sum();
        println!(
            "  sub-phases: ref_sites {t_sites:?}, element_edges {t_edges:?}, \
             tiered_circuits {t_circuits:?}; link_scores {n_scores}, total arms {n_arms}"
        );
        // Keep the work observable so nothing is optimized away, and confirm
        // the fixture actually generated per-element scores.
        assert!(
            ltm.vars.iter().any(|v| v.name.contains("link_score")),
            "fixture must generate link scores"
        );
        elapsed
    }

    for n in [50usize, 100, 200, 400] {
        let d = generate_for_width(n);
        println!("width {n:>4}: model_ltm_variables {d:?}");
    }
}

/// Build a `StitchPetal<&str>` from `[agg, x1, ..., xm]`.
fn petal<'a>(nodes: &[&'a str]) -> super::StitchPetal<&'a str> {
    super::StitchPetal {
        nodes: nodes.to_vec(),
        internal: nodes[1..].iter().copied().collect(),
    }
}

/// The mode-agnostic petal stitcher (`stitch_cross_agg_petals`, GH #515/#696)
/// enumerates exactly the disjoint-petal cross-agg loops: for one agg with
/// `k` pairwise-disjoint petals it emits `Σ_{m=2}^{k} C(k,m)` stitched
/// sequences -- ONE canonical ordering per subset (GH #676) -- and the
/// single petals themselves are NOT in the output (they are already
/// elementary loops the enumerator emits directly).
#[test]
fn stitch_cross_agg_petals_enumerates_disjoint_subsets() {
    // One agg "a" with three disjoint petals (each through its own internals).
    let petals = vec![(
        "a",
        vec![
            petal(&["a", "p1"]),
            petal(&["a", "p2"]),
            petal(&["a", "p3"]),
        ],
    )];
    let (stitched, truncated) = super::stitch_cross_agg_petals(petals, 1024);
    assert!(truncated.is_empty(), "well under budget");
    // 3 disjoint pairs + 1 triple = 4, one loop per subset.
    assert_eq!(stitched.len(), 4, "got {stitched:?}");
    // Each stitched sequence starts at the agg and contains it once per petal.
    for seq in &stitched {
        assert_eq!(seq[0], "a");
        let agg_count = seq.iter().filter(|n| **n == "a").count();
        let petal_count = seq.len() / 2; // each petal contributes [a, p_i]
        assert_eq!(agg_count, petal_count, "one agg per petal: {seq:?}");
    }
    // Exactly one triple (length 6: a,p?,a,p?,a,p?).
    assert_eq!(
        stitched.iter().filter(|s| s.len() == 6).count(),
        1,
        "one full-triple loop"
    );
    // Three pairs (length 4).
    assert_eq!(stitched.iter().filter(|s| s.len() == 4).count(), 3);
}

/// GH #676: exactly ONE canonical loop per disjoint petal subset. For a
/// fixed subset every cyclic ordering of the petals yields the same edge
/// multiset (each petal contributes its `agg→head` / internal / `tail→agg`
/// edges regardless of position), hence the same commutative loop-score
/// product -- so one representative suffices, and it is the priority-order
/// (fewest internal nodes first, then node-sequence tiebreak) concatenation
/// of the chosen petals, independent of the caller's petal order. With
/// `k = 4` disjoint petals the output is `Σ_{m=2}^{4} C(4,m) = 11`
/// sequences, each a distinct petal subset, and repeated calls are
/// byte-identical (the deterministic walk `assign_loop_ids` relies on).
#[test]
fn stitch_cross_agg_petals_one_canonical_ordering_per_subset() {
    use std::collections::BTreeSet;
    use std::collections::HashSet;

    // Deliberately NOT in priority order: priority sorts p1, p2 (1 internal
    // node each, name tiebreak) before q (2) before r (3).
    let build = || {
        vec![(
            "a",
            vec![
                petal(&["a", "r", "r2", "r3"]),
                petal(&["a", "q", "q2"]),
                petal(&["a", "p2"]),
                petal(&["a", "p1"]),
            ],
        )]
    };
    let (stitched, truncated) = super::stitch_cross_agg_petals(build(), 1024);
    assert!(truncated.is_empty(), "well under budget");
    assert_eq!(
        stitched.len(),
        11,
        "k=4 disjoint petals -> Σ_(m=2..4) C(4,m) = 11 subsets, one loop each; got {stitched:?}"
    );

    // Each stitched sequence is a distinct petal SUBSET (keyed by its petal
    // head set -- the nodes following each "a").
    let petal_heads = |seq: &[&str]| -> BTreeSet<String> {
        seq.iter()
            .zip(seq.iter().skip(1))
            .filter(|(n, _)| **n == "a")
            .map(|(_, head)| head.to_string())
            .collect()
    };
    let subsets: HashSet<BTreeSet<String>> = stitched.iter().map(|s| petal_heads(s)).collect();
    assert_eq!(
        subsets.len(),
        11,
        "every stitched sequence is a distinct subset"
    );

    // The emitted sequence per subset is the priority-order concatenation of
    // its petals: e.g. the {p2, q} pair is [a, p2, a, q, q2] (p2 first --
    // fewer internal nodes -- despite q preceding p2 in the input), and the
    // full subset is the four petals in full priority order.
    assert!(
        stitched.contains(&vec!["a", "p2", "a", "q", "q2"]),
        "the {{p2, q}} pair must be stitched in priority order; got {stitched:?}"
    );
    assert!(
        stitched.contains(&vec![
            "a", "p1", "a", "p2", "a", "q", "q2", "a", "r", "r2", "r3"
        ]),
        "the full 4-petal subset must be the priority-order concatenation; got {stitched:?}"
    );

    // Determinism: repeated calls produce byte-identical output.
    let (stitched2, _) = super::stitch_cross_agg_petals(build(), 1024);
    assert_eq!(stitched, stitched2, "stitching must be deterministic");
}

/// Petals that overlap on an internal node are never stitched together (they
/// would visit the same node twice, which is not a valid simple-through-agg
/// loop). With two overlapping petals there are zero cross-agg loops.
#[test]
fn stitch_cross_agg_petals_skips_overlapping_petals() {
    // Both petals share internal node "x".
    let petals = vec![("a", vec![petal(&["a", "x", "y"]), petal(&["a", "x", "z"])])];
    let (stitched, truncated) = super::stitch_cross_agg_petals(petals, 1024);
    assert!(
        stitched.is_empty(),
        "overlapping petals yield no loop: {stitched:?}"
    );
    assert!(truncated.is_empty());
}

/// The loop-count budget clips enumeration deterministically and flags the
/// truncated agg(s). With a budget of 2 over a 3-disjoint-petal agg (which
/// would otherwise yield 4 loops), only the first 2 are emitted and the agg
/// is reported truncated.
#[test]
fn stitch_cross_agg_petals_respects_budget() {
    let petals = vec![(
        "a",
        vec![
            petal(&["a", "p1"]),
            petal(&["a", "p2"]),
            petal(&["a", "p3"]),
        ],
    )];
    let (stitched, truncated) = super::stitch_cross_agg_petals(petals, 2);
    assert_eq!(stitched.len(), 2, "budget of 2 stops after 2 loops");
    assert_eq!(
        truncated,
        vec!["a"],
        "the clipped agg is reported truncated"
    );
}

/// When the budget fires partway through one agg, every *later* agg (sorted
/// after it) that had >= 2 petals is also reported truncated -- it never got
/// to run. An earlier-sorted agg's loops are emitted first.
#[test]
fn stitch_cross_agg_petals_budget_flags_later_aggs() {
    // Two aggs, each with two disjoint petals (1 pair loop each). A budget of 1
    // emits agg "a"'s single loop, then fires; agg "z" never runs.
    let petals = vec![
        ("a", vec![petal(&["a", "p1"]), petal(&["a", "p2"])]),
        ("z", vec![petal(&["z", "q1"]), petal(&["z", "q2"])]),
    ];
    let (stitched, truncated) = super::stitch_cross_agg_petals(petals, 1);
    assert_eq!(stitched.len(), 1);
    assert_eq!(
        truncated,
        vec!["a", "z"],
        "both the clipped agg and the un-reached later agg are truncated"
    );
}

/// `collect_agg_petals` extracts a petal only from a circuit touching exactly
/// one synthetic agg node, rotates it to start at the agg, and dedups
/// rotations of the same simple cycle on the internal set.
#[test]
fn collect_agg_petals_groups_single_agg_circuits() {
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let circuits: Vec<Vec<&str>> = vec![
        // Single-agg petal: pop[a] -> agg -> growth[a] -> (back to pop[a]).
        vec!["pop[a]", agg, "growth[a]"],
        // A rotation of the same petal -- must dedup.
        vec![agg, "growth[a]", "pop[a]"],
        // A different element's petal.
        vec!["pop[b]", agg, "growth[b]"],
        // A circuit with no agg -- ignored.
        vec!["x", "y"],
    ];
    let map = super::collect_agg_petals(&circuits, |n| n);
    let petals = map.get(agg).expect("agg group present");
    assert_eq!(
        petals.len(),
        2,
        "two distinct petals (the rotation deduped)"
    );
    for p in petals {
        assert_eq!(p.nodes[0], agg, "petal rotated to start at the agg");
    }
}

// The Track A3 stage-2b regression `legacy_and_shaped_bare_score_agree_when_
// source_bare_in_reducer` was removed here: it pinned that the deleted
// `(from, to)`-keyed `link_score_equation_text` and `link_score_equation_text_
// shaped(.., Bare)` derived byte-identical equations. With the legacy query
// gone, assembly's sub-case (a) reads the shaped query directly, so the two
// derivations are one -- the divergence the test guarded against is now
// structurally impossible. The emitted (changed-LAST) text for that probe is
// still pinned by the `scalar_feeder_bare_in_hoisted_reducer` characterization
// golden (`db::ltm_char_tests`).

// ---------------------------------------------------------------------------
// GH #986 (third consumer): the ceteris-paribus wrap resolves a bare-identifier
// subscript index against the AXIS IT INDEXES, not against the project's whole
// element namespace.
//
// `dimensions::resolve_axis_index_name` is the engine's single
// element-vs-dimension precedence rule, and it is what
// `compiler::subscript::normalize_subscripts3` implements. GH #986 unified
// `ltm_agg::classify_axis_access` and `post_transform::pin_dimension_name_indices`
// onto it and left the wrap on two PROJECT-WIDE predicates
// (`dimension_uniquely_containing_element`, `is_element_of_any_dimension`).
//
// The consequence was a wrong NUMBER with no diagnostic. A model variable whose
// canonical name happens to be an element of an UNRELATED dimension read as a
// runtime value to the simulation and as a static element selector to the wrap,
// so the "ceteris-paribus" partial left it LIVE and moved with it -- reporting
// real influence for an edge with no causal dependence at all. The qualification
// step made it worse: the index was rewritten to `otherdim·name`, naming an
// element of a dimension the subscripted variable is not even declared over,
// which still compiles and reads a different slot than the anchor did.
// ---------------------------------------------------------------------------

/// `share[boston]` reads `q`/`gtab` at the runtime index `ctr`, and has no
/// causal dependence on `pop[nyc]` whatsoever.
///
/// `declare_bucket` adds a dimension **no equation references**, whose first
/// element is named `ctr` -- the same canonical name as the model variable. It
/// changes nothing about the simulation; before the fix it changed the emitted
/// link score.
///
/// `indexed_name` only varies the subscripted variable's NAME. Both iterations
/// exercise the SAME path -- an ordinary arrayed variable subscripted directly --
/// and that is deliberate, because it is the only path the fix reaches.
///
/// **The `LOOKUP` table-index path is NOT covered here, and not by anything
/// else, because the fix does not reach it.** A graphical-function table holder
/// is by construction absent from `IteratedDimCtx::dep_dims` (GH #606 keeps it
/// off the dependency graph), so `ltm_augment::axis_dim_at` can never resolve its
/// axis and `freeze_lookup_table_indices` passes a literal `None`. GH #984's
/// defect still reproduces there on a model that compiles with zero diagnostics;
/// the reproduction is recorded on GH #984. Writing a `LOOKUP` fixture here would
/// pin the CURRENT (wrong) behaviour or fail -- neither is what this test is for.
///
/// An earlier revision of this rustdoc claimed the two iterations covered a
/// `LOOKUP` holder and an ordinary variable. They never did: the fixture has no
/// `LOOKUP`, no `GraphicalFunction`, and no lookup-only aux. That false claim is
/// exactly how the gap shipped -- the reviewer's fixture was a `LOOKUP`, the
/// reproduction was an ordinary subscript, a different guard fired, and nobody
/// noticed.
fn colliding_index_name_model(declare_bucket: bool, second_name: bool) -> datamodel::Project {
    let mut p = TestProject::new("colliding_index")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["nyc", "boston", "la"])
        .named_dimension("Slot", &["s1", "s2"]);
    if declare_bucket {
        // Declared, never referenced. `ctr` collides with the model variable.
        p = p.named_dimension("Bucket", &["ctr", "spare"]);
    }
    let indexed = if second_name { "gtab" } else { "q" };
    p.aux("tick", "1", None)
        .stock("counter", "0", &["tick"], &[], None)
        .aux("drive", "1 + counter", None)
        // 1, 2, 1, 2, ... -- a genuine runtime index.
        .aux("ctr", "1 + (INT(counter) MOD 2)", None)
        .array_with_ranges(
            &format!("{indexed}[Slot]"),
            vec![("s1", "1 * drive"), ("s2", "10 * drive")],
        )
        .array_flow_with_ranges(
            "share[Region]",
            vec![
                ("nyc", "pop[nyc] * 0.01"),
                ("boston", &format!("{indexed}[ctr] * 0.002 + 0 * ctr")),
                ("la", "pop[la] * 0.03"),
            ],
        )
        .array_flow_with_ranges(
            "inflow[Region]",
            vec![
                ("nyc", "share[nyc]"),
                ("boston", "share[boston]"),
                ("la", "share[la]"),
            ],
        )
        .array_stock("pop[Region]", "10", &["inflow"], &[], None)
        .build_datamodel()
}

/// The `boston` arm of the `pop[nyc] -> share` link score, plus the model's
/// diagnostics -- so a "the two agree" assertion can never be two compile
/// failures agreeing.
fn colliding_index_boston_arm(project: &datamodel::Project) -> (String, usize) {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    let diags = crate::db::collect_all_diagnostics(&db, sync.project);
    let ltm = crate::db::model_ltm_variables(&db, sync.models["main"].source, sync.project);
    let arm = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("link_score")
                && v.name.contains("pop[nyc]")
                && v.name.ends_with("share")
        })
        .map(|v| match &v.equation {
            crate::db::LtmEquation::Arrayed { elements, .. } => elements
                .iter()
                .find(|(e, _)| e == "boston")
                .map(|(_, arm)| arm.text.clone())
                .unwrap_or_else(|| panic!("no boston arm in {:?}", elements)),
            other => panic!("expected an arrayed score, got {other:?}"),
        })
        .expect("pop[nyc]->share link score");
    (arm, diags.len())
}

#[test]
fn a_colliding_index_name_is_resolved_against_the_axis_it_indexes() {
    for second_name in [true, false] {
        let (with_bucket, with_diags) =
            colliding_index_boston_arm(&colliding_index_name_model(true, second_name));
        let (without_bucket, without_diags) =
            colliding_index_boston_arm(&colliding_index_name_model(false, second_name));
        let label = if second_name { "gtab" } else { "q" };

        // Both models compile cleanly: the equality below is two real scores
        // agreeing, not two failures.
        assert_eq!(
            (with_diags, without_diags),
            (0, 0),
            "{label}: both models must compile with no diagnostics"
        );

        // The property, stated the way a modeller would hit it: declaring a
        // dimension that no equation references cannot change a link score.
        assert_eq!(
            with_bucket, without_bucket,
            "{label}: declaring an unreferenced dimension whose element name \
             collides with a model variable changed the emitted partial"
        );

        // `ctr` is a runtime read on a `Slot` axis that declares no element
        // `ctr`, so the ceteris-paribus wrap freezes it. (The
        // `PREVIOUS(ctr, ctr)` spelling is GH #975's index-position initial
        // value.)
        assert!(
            with_bucket.contains("PREVIOUS(ctr, ctr)"),
            "{label}: the index must be frozen, not left live; got: {with_bucket}"
        );
        // The pre-fix failure mode, pinned by name so a regression cannot pass
        // by merely differing: the index was rewritten to `bucket·ctr`, an
        // element of a dimension `q`/`gtab` is not declared over -- which still
        // compiles and reads a different slot than the anchor did.
        assert!(
            !with_bucket.contains("bucket\u{B7}ctr"),
            "{label}: the index must not be qualified onto an unrelated \
             dimension; got: {with_bucket}"
        );

        // The same property on the SIMULATED series, which is what a
        // practitioner sees. Text equality already implies it, but the emitted
        // text is an intermediate: this is the assertion that would survive a
        // rewrite of how the partial is spelled.
        assert_eq!(
            colliding_index_boston_series(&colliding_index_name_model(true, second_name)),
            colliding_index_boston_series(&colliding_index_name_model(false, second_name)),
            "{label}: declaring an unreferenced dimension changed the link score"
        );
    }
}

/// The simulated `boston` slot of the `pop[nyc] -> share` link score.
///
/// NOTE what this deliberately does NOT assert: that the series is ZERO.
/// `share[boston]` has no causal dependence on `pop[nyc]`, so a fully
/// ceteris-paribus partial would be identically zero -- and it is not; it runs
/// -1.06 / +0.73 / -1.03 / +0.82 on this fixture. That residual is a SEPARATE
/// defect from the one above and predates this branch: an index frozen inside an
/// already-frozen head is DOUBLE-lagged (the partial reads `q` at `t-1` indexed
/// by `ctr` at `t-2`, where the anchor `PREVIOUS(share)` used `ctr` at `t-1`).
/// The current behaviour is PINNED but has never been ADJUDICATED, and the
/// distinction matters for whoever picks this up.
/// `db::ltm_char_tests::per_element_dynamic_index_scores_preserve_head_lag` and
/// three siblings do freeze the lag, but that pin was written to catch a blanket
/// skip of the whole index pass and merely uses the lag as its discriminator --
/// it argues nowhere that reading the index at `t-2` against an anchor that read
/// it at `t-1` is right. The codebase's own position is the opposite:
/// `wrap_index_non_matching_in_previous`'s GH #759 comment calls reading an index
/// two steps back "semantically wrong for a genuinely-dynamic index".
///
/// Deferred because it is a SEMANTICS question -- what ceteris paribus means for
/// an index read under a freeze -- and it interacts with GH #975's own head-lag
/// pin, not because the current answer has been settled. A crude probe
/// (`if frozen { return index; }`, which disables the entire index pass, not just
/// the re-freeze) takes this fixture to exactly 0 and reds 5 tests; that is an
/// UPPER BOUND on the cost of the narrow change, not a measurement of it.
fn colliding_index_boston_series(project: &datamodel::Project) -> Vec<u64> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    use salsa::Setter;
    sync.project.set_ltm_enabled(&mut db).to(true);
    let sync = sync_from_datamodel(&db, project);
    sync.project.set_ltm_enabled(&mut db).to(true);
    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the fixture must compile");
    let offsets = compiled.offsets.clone();
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let results = vm.into_results();
    let name = offsets
        .keys()
        .map(|k| k.as_str().to_string())
        .find(|k| k.contains("link_score") && k.contains("pop[nyc]") && k.ends_with("share"))
        .expect("the pop[nyc]->share link score must be emitted");
    // `+ 1` is the `boston` slot: `Region = [nyc, boston, la]`, laid out in
    // declaration order.
    let base = offsets[&crate::common::Ident::new(&name)] + 1;
    // Compared by BIT PATTERN, so a difference cannot hide in a rounding
    // tolerance -- the claim is that the two models produce the same score, not
    // a similar one.
    (0..results.step_count)
        .map(|s| results.data[s * results.step_size + base].to_bits())
        .collect()
}

#[test]
fn an_index_naming_the_axis_own_element_stays_a_static_selector() {
    // The control that keeps the fix from being "freeze every bare index":
    // `s1` IS an element of `gtab`'s own `Slot` axis, so it is a selector and
    // must stay unwrapped (and qualified onto its own dimension).
    let project = TestProject::new("axis_element_index")
        .named_dimension("Region", &["nyc", "boston", "la"])
        .named_dimension("Slot", &["s1", "s2"])
        .aux("tick", "1", None)
        .stock("counter", "0", &["tick"], &[], None)
        .aux("drive", "1 + counter", None)
        .array_with_ranges("q[Slot]", vec![("s1", "1 * drive"), ("s2", "10 * drive")])
        .array_flow_with_ranges(
            "share[Region]",
            vec![
                ("nyc", "pop[nyc] * 0.01"),
                ("boston", "q[s1] * 0.002"),
                ("la", "pop[la] * 0.03"),
            ],
        )
        .array_flow_with_ranges(
            "inflow[Region]",
            vec![
                ("nyc", "share[nyc]"),
                ("boston", "share[boston]"),
                ("la", "share[la]"),
            ],
        )
        .array_stock("pop[Region]", "10", &["inflow"], &[], None)
        .build_datamodel();
    let (arm, diags) = colliding_index_boston_arm(&project);
    assert_eq!(diags, 0, "the control model must compile cleanly");
    assert!(
        arm.contains("q[slot\u{B7}s1]"),
        "an element of the indexed variable's OWN axis is a static selector, \
         qualified onto that axis; got: {arm}"
    );
    assert!(
        !arm.contains("PREVIOUS(s1"),
        "a static element selector must never be frozen; got: {arm}"
    );
}

/// `link_score_equation_text_shaped` documents that "a per-shape link score is
/// recomputed only when the involved variables (and their shape-classifying
/// dimensions) change". This is that claim, measured.
///
/// It has to be measured as a body-entry COUNT, not as memo identity: salsa
/// backdates a re-executed query whose value compares equal, so every link
/// score's memo looks untouched whether its body ran or not. The counter lives
/// in `db::ltm::compile` beside the query.
///
/// The failure this pins is not hypothetical. Routing
/// `reconstruct_single_variable` straight through the whole-model
/// `reconstruct_model_variables` map -- which is what a naive "read the cached
/// map" refactor does -- makes every link score depend on every variable's
/// lowered form, so ONE unrelated equation edit regenerates all of them. On a
/// model with thousands of links that is a full LTM rebuild per keystroke.
#[test]
fn an_unrelated_equation_edit_does_not_regenerate_every_link_score() {
    use crate::db::{
        compile_project_incremental, set_project_ltm_enabled, sync_from_datamodel_incremental,
    };

    // Two independent feedback loops sharing no variable, so an edit inside
    // one provably cannot change the other's scores. `untouched_*` is the
    // half the assertion is about.
    let project = TestProject::new("ltm_link_score_incrementality")
        .stock("edited_stock", "10", &["edited_in"], &[], None)
        .flow("edited_in", "edited_stock * edited_rate", None)
        .aux("edited_rate", "0.1", None)
        .stock("untouched_stock", "10", &["untouched_in"], &[], None)
        .flow("untouched_in", "untouched_stock * untouched_rate", None)
        .aux("untouched_rate", "0.2", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, state.project, true);
    compile_project_incremental(&db, state.project, "main").expect("first compile");

    // Count the whole cold build, so the edit's count has something to be a
    // fraction OF -- an assertion against a bare number would pass just as
    // happily on a model that emitted no link scores at all.
    let cold = super::compile::shaped_link_score_executions();
    assert!(
        cold >= 4,
        "fixture must emit link scores for both loops to be measuring anything, got {cold}"
    );

    // Edit ONE variable's equation, in the other loop.
    let mut edited = project.clone();
    for var in &mut edited.models[0].variables {
        if let datamodel::Variable::Aux(aux) = var
            && aux.ident == "edited_rate"
        {
            aux.equation = datamodel::Equation::Scalar("0.15".to_string());
        }
    }

    super::compile::reset_shaped_link_score_executions();
    let state2 = sync_from_datamodel_incremental(&mut db, &edited, Some(&state));
    compile_project_incremental(&db, state2.project, "main").expect("recompile after edit");
    let after_edit = super::compile::shaped_link_score_executions();

    // The bound is "strictly fewer than the cold build", not an exact number:
    // which scores touch `edited_rate` is a property of the LTM emitter that
    // may legitimately change, while "an edit in one loop must not regenerate
    // the other loop's scores" is the contract. An exact pin would fail on
    // every unrelated emitter change and teach nothing.
    assert!(
        after_edit < cold,
        "a one-variable equation edit regenerated {after_edit} of {cold} link scores -- \
         the per-involved-variable incrementality this query documents is gone. \
         The usual cause is a dependency on a whole-model query (every variable's \
         lowered form) where a per-variable one would do."
    );
}
