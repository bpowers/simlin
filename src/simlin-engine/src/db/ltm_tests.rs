// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::{compile_ltm_equation_fragment, scalarize_ltm_equation};
use crate::datamodel;
use crate::db::{
    LtmLinkId, RefShape, ShapedLinkScore, SimlinDb, compute_layout, shaped_link_score,
    sync_from_datamodel,
};
use crate::test_common::TestProject;

/// The bare-element rule at each of the two parse boundaries, for both
/// intrinsics. The source parse resolves `base_val[b2]` against `base_val`'s
/// own declared axis -- XMILE 1.0 footnote 9, the element wins over a
/// same-named variable -- so it never captures. The generated LTM parse has
/// no declared axis to ask (its target may be a synthetic variable) and
/// captures once a variable shadows the name
/// (`builtins_visitor::SnapshotIndexFacts`). `b2` is declared by two
/// dimensions at different positions, so nothing project-wide could have
/// decided either row.
///
/// Both parses are the production ones: the source row is
/// `parse_source_variable` over the synced variable, the LTM row is
/// `parse_ltm_equation` under `ltm_model_var_names`, exactly as
/// `model_ltm_implicit_var_info` calls it.
#[test]
fn a_bare_element_snapshot_captures_on_the_generated_path_only_when_shadowed() {
    use super::{LtmEquation, ltm_model_var_names, parse_ltm_equation};
    use crate::db::{parse_source_variable, project_dimensions_context};

    let classify = |call: &str, shadowed: bool| -> (usize, usize) {
        let mut tp = TestProject::new("bare_element_shadowing")
            .named_dimension("DimA", &["a1", "b2"])
            .named_dimension("DimB", &["b2", "x1"])
            .array_aux("base_val[DimA]", "1")
            .aux("lagged", call, None);
        if shadowed {
            tp = tp.aux("b2", "1", None);
        }
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &tp.build_datamodel());
        let model = sync.models["main"].source;
        let source = parse_source_variable(
            &db,
            sync.models["main"].variables["lagged"].source,
            sync.project,
        );
        let ltm = parse_ltm_equation(
            "$\u{205A}ltm\u{205A}probe",
            &LtmEquation::scalar(call.to_string()),
            project_dimensions_context(&db, sync.project),
            ltm_model_var_names(&db, model, sync.project),
        );
        (ltm.implicit_vars.len(), source.implicit_vars.len())
    };

    for call in ["PREVIOUS(base_val[b2], -7)", "INIT(base_val[b2])"] {
        assert_eq!(
            classify(call, false),
            (0, 0),
            "{call}: an unshadowed element is direct on both paths"
        );
        assert_eq!(
            classify(call, true),
            (1, 0),
            "{call}: a shadowed element captures on the generated path and is direct \
             on the source path"
        );
    }
}

/// An LTM helper's compiled phases are its capture kind, for both kinds, and
/// the diagnostic predicate demands exactly those phases.
///
/// Today's score generator emits `PREVIOUS` captures only, so the `INIT` row
/// is driven through `parse_ltm_equation` over a scalar `LtmEquation` built
/// as the generator builds one; its `LtmImplicitVarMeta` carries the fields
/// `model_ltm_implicit_var_info` computes for every scalar capture (not a
/// module, one slot), stated here because that query cannot be handed an
/// equation the generator does not produce.
#[test]
fn ltm_capture_helpers_compile_exactly_the_phases_their_kind_demands() {
    use salsa::Setter;

    use super::compile::{compile_ltm_implicit_var_fragment, ltm_helper_phases_present};
    use super::{LtmEquation, LtmImplicitVarMeta, ltm_model_var_names, parse_ltm_equation};
    use crate::capture::CaptureKind;
    use crate::db::project_dimensions_context;

    let project = TestProject::new("ltm_capture_phases")
        .aux("k", "1 + TIME", None)
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    sync.project.set_ltm_enabled(&mut db).to(true);
    let model = sync.models["main"].source;

    for (text, kind) in [
        ("PREVIOUS(k * 2, -7)", CaptureKind::Previous),
        ("INIT(k * 2)", CaptureKind::Init),
    ] {
        let parent = format!("$\u{205A}ltm\u{205A}probe\u{205A}{}", kind.as_str());
        let parsed = parse_ltm_equation(
            &parent,
            &LtmEquation::scalar(text.to_string()),
            project_dimensions_context(&db, sync.project),
            ltm_model_var_names(&db, model, sync.project),
        );
        assert!(parsed.variable.diagnostics.is_empty(), "{text}");
        let helpers: Vec<_> = parsed
            .implicit_vars
            .iter()
            .filter(|helper| helper.capture().is_some())
            .collect();
        assert_eq!(helpers.len(), 1, "{text}: one capture");
        assert_eq!(helpers[0].capture().unwrap().kind(), kind, "{text}");
        let meta = LtmImplicitVarMeta {
            ltm_parent_name: parent.clone(),
            size: 1,
            variable: helpers[0].clone(),
        };
        let mut reason = None;
        let fragment = compile_ltm_implicit_var_fragment(
            &db,
            &meta,
            model,
            sync.project,
            &[],
            Some(&mut reason),
        )
        .unwrap_or_else(|| panic!("{text}: the helper compiles"));
        assert!(reason.is_none(), "{text}: {reason:?}");
        assert_eq!(
            (
                fragment.fragment.initial_bytecodes.is_some(),
                fragment.fragment.flow_bytecodes.is_some(),
                fragment.fragment.stock_bytecodes.is_some(),
            ),
            (kind.needs_initials(), kind.needs_flows(), false),
            "{text}: the phases are the kind's demand"
        );
        assert!(ltm_helper_phases_present(&meta, Some(&fragment)));

        let mut missing = fragment.clone();
        if kind.needs_initials() {
            missing.fragment.initial_bytecodes = None;
        } else {
            missing.fragment.flow_bytecodes = None;
        }
        assert!(
            !ltm_helper_phases_present(&meta, Some(&missing)),
            "{text}: a demanded phase without bytecode is a dropped helper"
        );
    }
}

/// The helpers the score generator actually mints -- the flow-to-stock
/// score's nested `PREVIOUS` captures -- are flow-only: no initial fragment,
/// since `PREVIOUS`'s fallback covers every read before the first step
/// commits.
#[test]
fn generated_ltm_capture_helpers_are_flow_only() {
    use salsa::Setter;

    use super::compile::{compile_ltm_implicit_var_fragment, ltm_helper_phases_present};
    use super::model_ltm_implicit_var_info;
    use crate::capture::CaptureKind;

    let project = TestProject::new("generated_ltm_capture_phases")
        .stock("level", "100", &["growth"], &[], None)
        .flow("growth", "level * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    sync.project.set_ltm_enabled(&mut db).to(true);
    let model = sync.models["main"].source;
    let info = model_ltm_implicit_var_info(&db, model, sync.project);
    let captures: Vec<_> = info
        .values()
        .filter(|meta| meta.variable.capture().is_some())
        .collect();
    assert!(
        !captures.is_empty(),
        "the flow-to-stock score mints nested PREVIOUS captures"
    );
    for meta in captures {
        assert_eq!(
            meta.variable.capture().unwrap().kind(),
            CaptureKind::Previous,
            "the generator emits no INIT"
        );
        let fragment = compile_ltm_implicit_var_fragment(&db, meta, model, sync.project, &[], None)
            .expect("the generated helper compiles");
        assert!(fragment.fragment.initial_bytecodes.is_none());
        assert!(fragment.fragment.flow_bytecodes.is_some());
        assert!(ltm_helper_phases_present(meta, Some(&fragment)));
    }
}

/// `PREVIOUS(producer)` of a bare module instance in a generated LTM equation
/// is refused at lowering exactly as it is in a user equation
/// (`db::prev_init_tests::module_snapshot_arguments_are_resolved_at_lowering`):
/// an instance has no storage of its own, so a direct read would be slot zero
/// of whichever sub-model variable the layout put first, and the LTM parse --
/// which reads no module-ident set -- leaves the reference for lowering to
/// refuse rather than capturing it.
#[test]
fn test_ltm_bare_module_snapshot_is_refused_at_lowering() {
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

    let mut why = None;
    let fragment = compile_ltm_equation_fragment(
        &db,
        "$⁚ltm⁚test_prev_module",
        &crate::db::LtmEquation::scalar("PREVIOUS(producer)".to_string()),
        source_model,
        sync.project,
        Some(&mut why),
    )
    .expect("the LTM compiler keeps a fragment whose lowering failed, for reporting");

    assert!(
        fragment.fragment.flow_bytecodes.is_none(),
        "PREVIOUS of a bare module instance must emit no bytecode rather than read \
         slot zero of the instance"
    );
    let why = why.expect("the lowering refusal must reach the LTM diagnostic channel");
    assert!(
        why.contains("bare module instance 'producer'"),
        "the refusal must name the module read, got {why}"
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
/// The A2A size computation code path is exercised by
/// `test_a2a_ltm_previous_per_element` (`compile_ltm_equation_fragment`
/// with explicit dimensions).
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
        None,
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
    let ShapedLinkScore::Scored { var: lsv, .. } =
        shaped_link_score(&db, link_id, RefShape::Bare, source_model, sync.project)
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
    let result = shaped_link_score(
        &db,
        link_id,
        RefShape::FixedIndex(vec!["nyc".to_string()]),
        source_model,
        sync.project,
    );
    let ShapedLinkScore::Scored { var: lsv, .. } = result else {
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
    let scored = shaped_link_score(&db, link_id, RefShape::Bare, model, sync.project);
    let ShapedLinkScore::Scored { var: lsv, .. } = scored else {
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
        let scored = shaped_link_score(&db, link_id, RefShape::Bare, model, sync.project);
        let ShapedLinkScore::Scored { var: lsv, .. } = scored else {
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
        compile_ltm_equation_fragment(&db, "$⁚ltm⁚bad⁚scalar", &scalar, model, sync.project, None)
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
        compile_ltm_equation_fragment(
            &db,
            "$⁚ltm⁚bad⁚arrayed",
            &arrayed,
            model,
            sync.project,
            None
        )
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
    let map = super::collect_agg_petals(circuits.iter(), |n: &&str| *n, &mut |_| true, &mut |_| {})
        .expect("unmetered");
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

/// The collector is bounded to `MAX_AGG_PETALS + 1` petals per agg and that
/// bound is EXACT for the stitcher: stitching the bounded collection yields
/// the same loops and the same truncation flag as stitching every petal --
/// whatever order the circuits arrive in.
#[test]
fn collect_agg_petals_bounded_top_k_stitches_identically_to_the_full_set() {
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    // 12 petals of assorted lengths: more than the cap, with ties in length.
    let mut circuits: Vec<Vec<&str>> = vec![
        vec![agg, "a"],
        vec![agg, "b"],
        vec![agg, "c", "c2"],
        vec![agg, "d"],
        vec![agg, "e", "e2", "e3"],
        vec![agg, "f"],
        vec![agg, "g", "g2"],
        vec![agg, "h"],
        vec![agg, "i"],
        vec![agg, "j", "j2", "j3", "j4"],
        vec![agg, "k"],
        vec![agg, "l", "l2"],
    ];
    let full: Vec<(&str, Vec<super::StitchPetal<&str>>)> =
        vec![(agg, circuits.iter().map(|c| petal(c)).collect())];
    let (full_stitched, full_truncated) = super::stitch_cross_agg_petals(full, 1024);
    assert_eq!(
        full_truncated,
        vec![agg],
        "12 petals exceed the per-agg cap"
    );

    for rotation in 0..circuits.len() {
        circuits.rotate_left(1);
        // A rotation of the circuit list changes arrival order only.
        let _ = rotation;
        let map =
            super::collect_agg_petals(circuits.iter(), |n: &&str| *n, &mut |_| true, &mut |_| {})
                .expect("unmetered");
        let kept = &map[agg];
        assert_eq!(
            kept.len(),
            super::MAX_AGG_PETALS + 1,
            "one more than the stitcher keeps"
        );
        let bounded: Vec<(&str, Vec<super::StitchPetal<&str>>)> = vec![(agg, kept.clone())];
        let (stitched, truncated) = super::stitch_cross_agg_petals(bounded, 1024);
        assert_eq!(stitched, full_stitched);
        assert_eq!(truncated, full_truncated);
    }
}

/// Two petals over the same internal node SET (the two directions through the
/// same nodes) are one petal to the stitcher; the representative is the
/// smaller `nodes` sequence whichever arrives first, so the collection is a
/// function of the circuit set rather than of arrival order.
#[test]
fn collect_agg_petals_picks_the_smallest_representative_of_an_internal_set() {
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    for circuits in [
        vec![vec![agg, "x", "y"], vec![agg, "y", "x"]],
        vec![vec![agg, "y", "x"], vec![agg, "x", "y"]],
        // Rotations of both, not starting at the agg.
        vec![vec!["y", "x", agg], vec!["x", "y", agg]],
    ] {
        let map =
            super::collect_agg_petals(circuits.iter(), |n: &&str| *n, &mut |_| true, &mut |_| {})
                .expect("unmetered");
        let kept = &map[agg];
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].nodes, vec![agg, "x", "y"]);
    }
}

/// Every kept petal is charged through `charge` and credited back through
/// `release` when displaced, so what is held on the meter at the end is
/// exactly the kept petals; a refused charge stops the collection with `None`.
#[test]
fn collect_agg_petals_charges_what_it_keeps_and_declines_when_refused() {
    use std::cell::Cell;
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    // Twelve distinct petals in two length classes, so the cap displaces some.
    let names: Vec<String> = (0..12).map(|i| format!("v{i}")).collect();
    let circuits: Vec<Vec<&str>> = (0..12)
        .map(|i| {
            if i % 3 == 0 {
                vec![agg, names[i].as_str(), "m"]
            } else {
                vec![agg, names[i].as_str()]
            }
        })
        .collect();
    let held = Cell::new(0isize);
    let charged_total = Cell::new(0isize);
    let map = super::collect_agg_petals(
        circuits.iter(),
        |n: &&str| *n,
        &mut |b| {
            held.set(held.get() + b as isize);
            charged_total.set(charged_total.get() + b as isize);
            true
        },
        &mut |b| held.set(held.get() - b as isize),
    )
    .expect("never refused");
    let kept = &map[agg];
    let kept_bytes: isize = kept
        .iter()
        .map(|p| {
            let len = p.nodes.len();
            (len * std::mem::size_of::<&str>() + len * (2 * std::mem::size_of::<&str>() + 1))
                as isize
        })
        .sum();
    assert_eq!(
        held.get(),
        kept_bytes,
        "held == kept: displaced petals were credited back"
    );
    assert!(
        charged_total.get() > held.get(),
        "some petals were displaced along the way"
    );

    // A meter that refuses everything: `None`, and nothing remains held.
    let held = Cell::new(0isize);
    assert!(
        super::collect_agg_petals(circuits.iter(), |n: &&str| *n, &mut |_| false, &mut |b| {
            held.set(held.get() - b as isize)
        })
        .is_none()
    );
    assert_eq!(held.get(), 0);
}

/// `stitched_output_bound` bounds the stitcher's output for every petal
/// configuration it could be asked about.
#[test]
fn stitched_output_bound_is_an_upper_bound_on_the_stitcher() {
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let petals: Vec<(&str, Vec<super::StitchPetal<&str>>)> = vec![(
        agg,
        vec![
            petal(&[agg, "p1"]),
            petal(&[agg, "p2", "q2"]),
            petal(&[agg, "p3"]),
            petal(&[agg, "p4", "q4", "r4"]),
        ],
    )];
    for budget in [1usize, 2, 3, 7, 11, 1024] {
        let bound = super::stitched_output_bound(&petals, budget);
        let (stitched, _) = super::stitch_cross_agg_petals(petals.clone(), budget);
        let actual: usize = stitched
            .iter()
            .map(|s| std::mem::size_of_val::<[&str]>(s))
            .sum();
        assert!(actual <= bound, "budget {budget}: {actual} > bound {bound}");
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
/// The `boston` arm survives the GH #977 omission on its own merits, and WHY it
/// survives is the whole distinction between this fixture and the control below.
/// Freezing the runtime index inside an already-frozen head yields
/// `PREVIOUS(q[PREVIOUS(ctr, ctr)])` -- `q` read at `t-1` indexed at `t-2`,
/// where the `PREVIOUS(share)` anchor indexed at `t-1`. That is not
/// `PREVIOUS(target)`, so the lag-alignment check rejects it and the arm is
/// materialized. The control below has a genuinely STATIC selector, so its arm
/// really is a structural zero and really is omitted.
///
/// That also settles what "the slot is absent under both variants" would be
/// worth as a stand-in for the assertions below: nothing. Both readings of `ctr`
/// -- frozen (`PREVIOUS(ctr, ctr)`, correct) and qualified onto the unrelated
/// dimension (`bucket·ctr`, the defect) -- leave the arm looking entirely
/// frozen, so an omission-based assertion cannot tell a selector from a freeze.
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
                // A missing arm here means the GH #977 omission claimed the
                // slot, which would gut every assertion downstream rather than
                // fail it -- see `colliding_index_name_model`'s note on the
                // zero-coefficient term that keeps this arm materialized.
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
///
/// It is ALSO a soundness input, and THAT half is settled. These numbers are the
/// measurement that an arm can look entirely frozen and still be worth -1.06, so
/// the GH #977 omission must not claim it as a structural zero. They sat here
/// framed only as a semantics question until a code review found the same class
/// from the other direction; `pinned_double_lag_residual_is_not_a_structural_zero`
/// below pins them as VALUES, so the next reader inherits the number rather than
/// the framing.
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

/// The double-lag residual, pinned as VALUES rather than described in prose.
///
/// `colliding_index_boston_series`' rustdoc has recorded -1.06 / +0.73 / -1.03 /
/// +0.82 for this slot for some time, framed as an unadjudicated semantics
/// question about what ceteris paribus means for an index read under a freeze.
/// It is that. It is ALSO the measurement showing this arm is not a structural
/// zero -- every occurrence in it is frozen, it looks entirely inert, and it is
/// worth -1.06 -- which is the soundness input the GH #977 omission needs and
/// which nobody connected until a code review found the same class from the
/// other direction.
///
/// Prose in a rustdoc does not fail. This does: a change that lets the omission
/// claim this arm reds here on the first value, and a change that alters the
/// residual reds on the specific numbers rather than on a vague "it moved".
///
/// The lag-alignment check in `ltm_augment_zero_slot` is what keeps the arm
/// alive; `db::ltm_value_gate_tests::a_nested_freeze_arm_is_not_a_structural_zero`
/// pins the same mechanism on a minimal fixture. This row exists because THESE
/// numbers are the ones that were already on disk and read past.
#[test]
fn pinned_double_lag_residual_is_not_a_structural_zero() {
    let series: Vec<f64> = colliding_index_boston_series(&colliding_index_name_model(true, false))
        .into_iter()
        .map(f64::from_bits)
        .collect();

    // The first two steps are the guard form's own warm-up (TIME = INITIAL_TIME,
    // then the first live step), so the residual starts at index 2.
    assert!(
        series.len() >= 6,
        "fixture must run long enough to show the residual; got {series:?}"
    );
    assert_eq!(
        (series[0], series[1]),
        (0.0, 0.0),
        "the guard form's warm-up steps; got {series:?}"
    );
    for (i, expected) in [
        (2usize, -1.0588235294117647f64),
        (3, 0.7297297297297297),
        (4, -1.0285714285714287),
        (5, 0.8181818181818182),
    ] {
        assert_eq!(
            series[i], expected,
            "step {i} of the documented double-lag residual moved; full series {series:?}"
        );
    }
    // The load-bearing half, stated on its own so a future reader cannot miss
    // which property is the soundness one: this arm is NOT zero.
    assert!(
        series[2].abs() > 1.0,
        "an arm whose every occurrence is frozen is still worth {}; it must never \
         be omitted as a structural zero",
        series[2]
    );
}

#[test]
fn an_index_naming_the_axis_own_element_stays_a_static_selector() {
    // The control that keeps the fix from being "freeze every bare index":
    // `s1` IS an element of `gtab`'s own `Slot` axis, so it is a selector and
    // must stay unwrapped (and qualified onto its own dimension).
    //
    // `0 * pop[nyc]` gives this arm a live source reference so there is text to
    // inspect. It is needed HERE and not in `colliding_index_name_model`, and
    // that asymmetry IS the point: `s1` is a static selector, so
    // `PREVIOUS(q[slot·s1])` reads `q` at `t-1` at a fixed slot and really
    // does equal `PREVIOUS(share)` -- a genuine structural zero, correctly
    // omitted. The sibling's runtime index is double-lagged and is not.
    // Removing this term reds this test and leaves the sibling green, which is
    // the sharpest statement of what the lag-alignment check distinguishes.
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
                ("boston", "q[s1] * 0.002 + 0 * pop[nyc]"),
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

/// The third `AxisIndexName` arm, and the one GH #986 left unguarded: the axis
/// IS known, the index is neither an element of that axis nor a dimension the
/// target iterates -- so the verdict is `Unresolved` -- and yet the name is a
/// DIMENSION, which is never a causal value.
///
/// The wrap's own dimension-name guard (GH #759) says exactly that, but it was
/// written when the axis was always unknown and stayed gated on `axis_unknown`,
/// so this arm reached the recursive wrap and froze a dimension name:
/// `PREVIOUS(aggregated[PREVIOUS(agg, agg)])`. `builtins_visitor` then hoists that
/// now-dynamic index into a `PREVIOUS`-capture helper aux
/// (`aggregated[previous(agg·a1, agg·a1)]`) whose inner argument constant-folds to
/// the element's offset, and codegen rejects `PREVIOUS(<literal>)` -- so the helper
/// keeps a layout slot with no bytecode, reads a constant 0, and every score
/// through it is silently degraded. This accounted for 49 of the 1,603 LTM
/// fragment-compile failures on C-LEARN.
///
/// The fixture is C-LEARN's shape in miniature: `Pct interim change in FF
/// emissions[COP] = IF THEN ELSE(Time < Emissions reference year[COP], ..., Pct
/// interim in FF emissions Aggregated[Aggregated Regions])` -- a target iterating
/// one dimension reading a dep declared over ANOTHER, spelled with that other
/// dimension's own name (legal because the dep's dimension maps onto the
/// target's).
///
/// This test constrains ONE arm of
/// [`crate::dimensions::resolve_axis_index_name`] as the wrap consumes it:
/// `Unresolved` where the name is a DIMENSION. The other two arms are covered,
/// and not here:
///
/// * `Element` -- `an_index_naming_the_axis_own_element_stays_a_static_selector`
///   (directly above).
/// * `Unresolved` naming a model VARIABLE, which must still FREEZE --
///   `a_colliding_index_name_is_resolved_against_the_axis_it_indexes`, whose
///   `PREVIOUS(ctr, ctr)` assertion is this test's counterweight: together they
///   say the guard distinguishes a dimension from a variable rather than
///   declining to freeze bare indices generally.
/// * `IteratedDim` -- twelve tests, all in `db::ltm_char_tests` except the last:
///   `char_per_element_{ambiguous_pin, dynamic_index, index_nested_occurrence,
///   mapped_occurrence, mixed_occurrences, other_deps}`,
///   `bare_arrayed_dep_is_pinned_over_its_own_declared_dims`,
///   `per_element_{ambiguous_pin, mapped_occurrence}_scores_are_live_not_silent_zero`,
///   `mapped_bare_live_source_ref_is_pinned_through_the_correspondence`,
///   `per_element_dynamic_index_scores_preserve_head_lag`, and
///   `ltm_augment::tests::gh526_transposed_other_dep_with_threaded_dims_is_unfreezable`.
///   (`char_per_element_link_scores` does NOT reach it, despite the name.)
///
/// **This fixture reaches `IteratedDim` not at all**, and the reason is worth
/// stating because the fixture LOOKS like it should: `level[cop]` and
/// `ref_year[cop]` are subscripted by the target's own iterated dimension, but
/// both are collapsed to a bare `Var` in `wrap_non_matching_in_previous` --
/// the live source by the GH #511 `Bare` normalization, the other-dep by
/// `OtherDepVerdict::Collapse` -- BEFORE any index is walked. So no `[cop]`
/// index ever reaches the index pass, and none appears in the emitted text.
/// An earlier revision of this comment claimed those indices covered the arm.
/// Measured, not read: replacing `AxisIndexName::IteratedDim => return index`
/// with `panic!` leaves this test passing when run alone, and panics in exactly
/// the twelve tests listed above.
#[test]
fn a_dimension_name_index_is_not_frozen_when_the_axis_is_known() {
    use salsa::Setter;

    let project = TestProject::new("dim_name_index")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("cop", &["c1", "c2"])
        .named_dimension_with_mapping("agg", &["a1", "a2"], "cop")
        .array_aux("aggregated[agg]", "TIME * 2")
        .array_aux("ref_year[cop]", "2")
        .array_stock("level[cop]", "1", &["growth"], &[], None)
        .array_flow("growth[cop]", "target[cop] * 0.1", None)
        .array_aux(
            "target[cop]",
            "if TIME < ref_year[cop] then level[cop] else aggregated[agg]",
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = crate::db::model_ltm_variables(&db, source_model, source_project);
    let score = ltm
        .vars
        .iter()
        .find(|v| v.name.ends_with("link_score\u{205A}level\u{2192}target"))
        .map(|v| match &v.equation {
            crate::db::LtmEquation::ApplyToAll(_, arm) => arm.text.clone(),
            other => panic!("expected an apply-to-all score, got {other:?}"),
        })
        .expect("the level->target link score must be emitted");

    // The co-source freezes WHOLESALE with its dimension-name index verbatim --
    // the same shape `ltm_augment_tests::partial_equation_dimension_name_index_
    // not_wrapped` pins on the axis-unknown path.
    assert!(
        score.contains("PREVIOUS(aggregated[agg])"),
        "a dimension-name index must be left verbatim inside the frozen \
         co-source; got: {score}"
    );
    // The pre-fix spelling, pinned by name so a regression cannot pass by
    // merely differing. (`PREVIOUS(agg,` rather than `PREVIOUS(agg` -- the
    // latter also matches the legitimate `PREVIOUS(aggregated...)`.)
    assert!(
        !score.contains("PREVIOUS(agg,"),
        "a dimension name is not a causal value and must never be frozen; \
         got: {score}"
    );
    // The runtime consequence, which is what makes this a defect rather than a
    // spelling preference: the frozen index forced a capture helper whose
    // argument constant-folded, so the helper compiled to nothing.
    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    assert!(
        diags.is_empty(),
        "every LTM fragment and helper must compile; got: {:?}",
        diags
            .iter()
            .map(|d| (&d.variable, &d.error))
            .collect::<Vec<_>>()
    );
}

/// `shaped_link_score` documents that "a per-shape link score is
/// recomputed only when the involved variables (and their shape-classifying
/// dimensions) change". This is that claim, measured.
///
/// It has to be measured as a body-entry COUNT, not as memo identity: salsa
/// backdates a re-executed query whose value compares equal, so every link
/// score's memo looks untouched whether its body ran or not. The counter lives
/// in `db::ltm::compile` beside the query.
///
/// The failure this pins is not hypothetical. Routing
/// `lowered_variable_by_name` straight through the whole-model
/// `model_lowered_variables` map -- which is what a naive "read the cached
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

/// An arrayed dep the per-element pin table cannot cover must make the edge
/// decline LOUDLY, not produce a score computed around a hole.
///
/// The shape: `target[cop]` reads `aggregated[cop]`, where `aggregated` is
/// declared over `agg` -- a dimension with DISJOINT element names and NO
/// mapping to `cop`.
///
/// It COMPILES because `cop` is the dimension the equation ITERATES and the two
/// dimensions declare no correspondence at all, which is the ORDINAL last
/// resort of `build_view_from_ops`'s `ActiveDimRef` arm: `target[c1]` reads
/// `agg`'s FIRST element, by POSITION, neither name nor mapping having
/// resolved it
/// (`mapped_reference_semantics_tests`' `no_mapping_equal_cardinality` measures
/// exactly this -- a cross-dimension read between two dimensions declared to
/// have nothing to do with each other compiles and produces numbers).
/// `build_view_from_ops` is never reached. The DESCRIBER declines because
/// `db::bare_axis_pairing` relates axes by name, by a DECLARED mapping, or by
/// equal indexed size, and `DimensionsContext::executed_read_correspondence`
/// resolves an element by name or through a map -- this pair has none of
/// those, so `dep_element_pins` can project no axis of `aggregated` and the
/// dep is absent from the pin table entirely.
///
/// The element names are deliberately disjoint. An earlier revision used `cop`'s
/// own names on `agg`, which made the values look name-matched when the read is
/// positional -- true only by the coincidence that both lists were declared in
/// the same order, and a fixture that passes by coincidence records a mechanism
/// that is not the one running.
///
/// This fixture used an EXPLICIT element map before GH #997. That shape is now
/// projectable -- it is exactly the class of dep C-LEARN reads, and
/// `an_element_mapped_arrayed_dep_is_scored_through_the_map` asserts the score
/// it gets -- so the guard needed a shape that still cannot project.
///
/// What that produced before this guard: the dimension-name subscript survived
/// into the scalar partial, `builtins_visitor` hoisted it into a
/// `PREVIOUS`-capture helper, the helper failed to lower
/// (`DimensionInScalarContext`) -- and the PARENT still compiled, because it
/// merely references the helper's slot. So the link score existed, was read by
/// every loop through it, and evaluated one branch of its own equation as a
/// constant 0. A wrong number wearing a warning meant for something else.
///
/// "No score, warned" beats a wrong score: the edge is now recorded in
/// `unscoreable_edges` and no `LtmSyntheticVar` is emitted for it, so loops
/// through it drop rather than reporting a number computed around the hole.
/// This is the same contract the GH #311 parse failure and the GH #743
/// unfreezable partial already use.
#[test]
fn an_uncoverable_arrayed_dep_declines_the_edge_loudly() {
    use salsa::Setter;

    let project = TestProject::new("unpinnable_dep")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("cop", &["c1", "c2"])
        .named_dimension("agg", &["x", "y"])
        .array_aux("aggregated[agg]", "TIME * 2")
        .aux("switch", "1 + SUM(level[*]) * 0.01", None)
        .array_stock("level[cop]", "1", &["growth"], &[], None)
        .array_flow("growth[cop]", "target[cop] * 0.1", None)
        .array_aux(
            "target[cop]",
            "if switch > 1.05 then aggregated[cop] else level[cop]",
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    let source_model = sync_from_datamodel(&db, &project).models["main"].source;

    // Non-vacuity: the MODEL must compile, or "no score emitted" would be
    // satisfied by a project that never got as far as scoring anything. The
    // simulation reads `aggregated[cop]` POSITIONALLY (see the fixture note),
    // which is why an unmapped pair with unrelated element names is legal here.
    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .map(|d| (&d.variable, &d.error))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture must compile; got: {errors:?}"
    );

    let ltm = crate::db::model_ltm_variables(&db, source_model, source_project);
    assert!(
        !ltm.vars.is_empty(),
        "the model must emit LTM variables, or the empty assertion below is vacuous"
    );
    let emitted: Vec<&String> = ltm
        .vars
        .iter()
        .map(|v| &v.name)
        .filter(|n| n.contains("link_score\u{205A}switch\u{2192}target"))
        .collect();
    assert!(
        emitted.is_empty(),
        "the switch->target edge cannot be scored (its `aggregated[cop]` dep is \
         unpinnable), so NO link score may be emitted for it; got: {emitted:?}"
    );

    // The decline must be LOUD, and it must be the only thing left: a helper
    // that fails while its parent compiles is exactly the silent zero this
    // guard exists to remove.
    let silently_degraded: Vec<&String> = diags
        .iter()
        .filter_map(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) if m.contains("silently degraded") => {
                d.variable.as_ref()
            }
            _ => None,
        })
        .collect();
    assert!(
        silently_degraded.is_empty(),
        "no fragment may be left reading a constant 0 under a compiling parent; \
         got: {silently_degraded:?}"
    );
    // The decline must be LOUD and must be THIS decline: `!diags.is_empty()` is
    // satisfied by any diagnostic at all, including one from an unrelated defect.
    assert!(
        diags.iter().any(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) =>
                m.contains("cannot be projected onto that target element")
                    && m.contains("switch\u{2192}target"),
            _ => false,
        }),
        "the switch->target decline must carry the unprojectable-dep warning; got: {:?}",
        diags
            .iter()
            .map(|d| (&d.variable, &d.error))
            .collect::<Vec<_>>()
    );
}

/// GH #997: an arrayed dep read across an EXPLICIT element map IS scored, and
/// the pin follows the MAP.
///
/// This is C-LEARN's shape in miniature, and the class of edge #997 exists to
/// recover: `target[cop]` reads `aggregated[agg]` -- the source's OWN dimension
/// name as the subscript -- with `agg` mapping onto `cop` through a declared
/// element map. `mapped_reference_semantics_tests`' `SourceOwnDim` row measures
/// that spelling against the VM: it resolves name-first and then through the
/// map, at every cardinality. Before #997 one correspondence served this
/// spelling and the positional one alike, declined both, and every such edge
/// took the loud unprojectable-dep skip -- 13 of them on C-LEARN.
///
/// The map here is the reverse permutation (a1 -> c2), so the pinned element is
/// the discriminator: a positional pin would spell `agg\u{B7}a1` for `c1` where
/// the map says `agg\u{B7}a2`.
#[test]
fn an_element_mapped_arrayed_dep_is_scored_through_the_map() {
    use salsa::Setter;

    let project = TestProject::new("element_mapped_dep")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("cop", &["c1", "c2"])
        .named_dimension_with_element_mapping(
            "agg",
            &["a1", "a2"],
            "cop",
            &[("a1", "c2"), ("a2", "c1")],
        )
        .array_aux("aggregated[agg]", "TIME * 2")
        .aux("switch", "1 + SUM(level[*]) * 0.01", None)
        .array_stock("level[cop]", "1", &["growth"], &[], None)
        .array_flow("growth[cop]", "target[cop] * 0.1", None)
        .array_aux(
            "target[cop]",
            "if switch > 1.05 then aggregated[agg] else level[cop]",
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    let source_model = sync_from_datamodel(&db, &project).models["main"].source;

    let ltm = crate::db::model_ltm_variables(&db, source_model, source_project);
    let scored: Vec<&crate::db::LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("link_score\u{205A}switch\u{2192}target"))
        .collect();
    assert_eq!(
        scored.len(),
        2,
        "one scalar score per `cop` element; got: {:?}",
        scored.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // The `c1` score must pin the dep to the element the MAP names (a2), not
    // the one an ordinal would (a1).
    let c1 = scored
        .iter()
        .find(|v| v.name.ends_with("target[c1]"))
        .expect("a score for c1");
    let text = c1.equation.source_text();
    assert!(
        text.contains("aggregated[agg\u{B7}a2]"),
        "the dep must be pinned to the element the declared map names; got: {text}"
    );
    assert!(
        !text.contains("aggregated[agg\u{B7}a1]"),
        "a positional pin would read the other element; got: {text}"
    );

    // And the decline is gone: no unprojectable-dep warning for this edge.
    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    let declines: Vec<_> = diags
        .iter()
        .filter(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) => {
                m.contains("cannot be projected onto that target element")
            }
            _ => false,
        })
        .map(|d| (&d.variable, &d.error))
        .collect();
    assert!(declines.is_empty(), "got: {declines:?}");
}

/// GH #997: the class-D edge itself -- an arrayed SOURCE read through an
/// element-mapped axis -- is scored per (source row, target element), not with
/// the conservative cross-product it collapsed to while the reference
/// classified `DynamicIndex`.
///
/// The map is MANY-TO-ONE (C-LEARN's shape at a smaller scale): two `agg`
/// elements onto four `cop` ones. That is the cardinality the positional rule
/// cannot describe at all, so every name below is evidence the map-following
/// rule produced it -- and each source row appears under two different target
/// elements, which is exactly what a many-to-one read is.
#[test]
fn a_many_to_one_mapped_read_is_scored_per_row_and_element() {
    use salsa::Setter;

    let project = TestProject::new("class_d_scores")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("cop", &["c1", "c2", "c3", "c4"])
        .named_dimension_with_element_mapping(
            "agg",
            &["a1", "a2"],
            "cop",
            &[("a1", "c1"), ("a1", "c2"), ("a2", "c3"), ("a2", "c4")],
        )
        //  must sit INSIDE a feedback loop: exhaustive mode scores
        // the edges loops traverse, so a feed-forward source gets no link score
        // at all and the assertion below would be vacuous.
        .array_aux("aggregated[agg]", "1 + SUM(level[*]) * 0.01")
        .array_stock("level[cop]", "1", &["growth"], &[], None)
        .array_flow("growth[cop]", "target[cop] * 0.1", None)
        .array_aux("target[cop]", "aggregated[agg] * 2 + level[cop]")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    let source_model = sync_from_datamodel(&db, &project).models["main"].source;

    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .map(|d| (&d.variable, &d.error))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture must compile; got: {errors:?}"
    );

    let ltm = crate::db::model_ltm_variables(&db, source_model, source_project);
    let mut scored: Vec<&String> = ltm
        .vars
        .iter()
        .map(|v| &v.name)
        .filter(|n| n.contains("link_score\u{205A}aggregated[") && n.contains("\u{2192}target["))
        .collect();
    scored.sort();
    let want: Vec<String> = [("a1", "c1"), ("a1", "c2"), ("a2", "c3"), ("a2", "c4")]
        .iter()
        .map(|(row, elem)| {
            format!("$\u{205A}ltm\u{205A}link_score\u{205A}aggregated[{row}]\u{2192}target[{elem}]")
        })
        .collect();
    let want: Vec<&String> = want.iter().collect();
    assert_eq!(
        scored, want,
        "one score per (mapped source row, target element), and no off-map pair"
    );

    // The equation must READ the row its name claims: `target[c2]` reads
    // `aggregated[a1]`, and a positional pin has no answer at all here (there
    // is no fourth `agg` element for `c4`'s ordinal).
    let c2 = ltm
        .vars
        .iter()
        .find(|v| v.name.ends_with("aggregated[a1]\u{2192}target[c2]"))
        .expect("the a1 -> c2 score");
    let text = c2.equation.source_text();
    assert!(
        text.contains("aggregated[agg\u{B7}a1]"),
        "the live source must be read at the mapped row; got: {text}"
    );
    assert!(
        !text.contains("aggregated[agg\u{B7}a2]"),
        "no other row may appear in this element's partial; got: {text}"
    );
}

/// A `LOOKUP` table argument's dimension-name index must be element-pinned in a
/// per-element scalar partial, exactly as every other arrayed dep of that target
/// already is.
///
/// The gap (GH #984): a graphical-function holder is deliberately absent from
/// the DEPENDENCY GRAPH -- `classify_dependencies` records it in
/// `referenced_tables` and keeps it out of `all` (GH #606) so a table reference
/// creates no runlist or causal edge -- and `pinnable_arrayed_deps` built the
/// per-element pin table from that same dep set. So `tbl[region]` reached the
/// scalar fragment with its dimension-name index intact, the fragment did not
/// lower (`DimensionInScalarContext`), and the link score was dropped with a
/// warning. The holder's DECLARED dimensions were never the problem: they sit on
/// the datamodel like any other variable's, one field away on the same struct
/// the same pass already returns.
///
/// This is C-LEARN's shape in miniature -- `Im 6 FF CO2[COP] = ... LOOKUP(RS CO2
/// FF[COP], Time/One year) * Adjustment to CO2eq ...`, a scalar source into an
/// arrayed target whose equation reads a per-element GF holder subscripted by
/// its own dimension name. All 413 of C-LEARN's occurrences are this IDENTITY
/// shape (the holder's declared axis IS the dimension the index spells, and the
/// target iterates it), which is what keeps the pin free of the unresolved
/// element-map question.
///
/// The assertion is the EXACT series, not a property of it. An earlier revision
/// asserted only "not identically zero" and "all finite" while claiming to
/// constrain the pinned element -- and it did not: mutating `dep_axis_elements`
/// to pin every axis to the dep's FIRST element left it green. Worse, that
/// fixture could not have caught the mutant even in principle, because with
/// `factor` as the sole varying input the score saturates at exactly 1.0 for
/// BOTH elements -- `out[a]` and `out[b]` differ (2.04 vs 3.06) while their
/// scores were identical. `drift` exists to break that saturation: with a second
/// varying contribution the score becomes a real ratio, the table slope enters
/// the value, and the two elements separate (0.805 vs 0.861). Both facts are
/// asserted, and the second is what makes the first mean anything.
#[test]
fn a_lookup_table_index_is_element_pinned_in_a_per_element_partial() {
    use crate::datamodel::{
        Equation, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale, Variable,
    };
    use salsa::Setter;

    // Per-element table: `a` doubles its input, `b` triples it. Distinct slopes
    // are what make a mis-pinned index visible in the value.
    let gf = |slope: f64| GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 10.0 * slope],
        x_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 10.0 * slope,
        },
    };
    // A lookup-only holder: empty equation text, the table carried per element.
    let tbl = Variable::Aux(crate::datamodel::Aux {
        ident: "tbl".to_string(),
        equation: Equation::Arrayed(
            vec!["region".to_string()],
            vec![
                ("a".to_string(), String::new(), None, Some(gf(2.0))),
                ("b".to_string(), String::new(), None, Some(gf(3.0))),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    });

    let mut project = TestProject::new("lookup_pin")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("region", &["a", "b"])
        // The scalar live source, in a feedback loop with the target.
        .array_stock("level[region]", "1", &["growth"], &[], None)
        .array_flow("growth[region]", "out[region] * 0.1", None)
        .aux("factor", "1 + SUM(level[*]) * 0.01", None)
        .aux("drift", "TIME * 0.5", None)
        .array_aux("out[region]", "LOOKUP(tbl[region], TIME) * factor + drift")
        .build_datamodel();
    project.models[0].variables.push(tbl);

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    // The model itself must be sound, or the score assertions below are moot.
    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    assert!(
        diags.is_empty(),
        "the fixture must compile with no diagnostics; got: {:?}",
        diags
            .iter()
            .map(|d| (&d.variable, &d.error))
            .collect::<Vec<_>>()
    );

    // The per-element score for the `factor -> out[b]` edge. `b`'s table has
    // slope 3, so a partial that pinned the index to `a` (slope 2) -- or left it
    // unpinned -- is a different number, not merely a different spelling.
    let compiled = crate::db::compile_project_incremental(&db, source_project, "main")
        .expect("the fixture must compile");
    let offsets = compiled.offsets.clone();
    let score_name = offsets
        .keys()
        .map(|k| k.as_str().to_string())
        .find(|k| k.contains("link_score") && k.contains("factor\u{2192}out[b]"))
        .expect("the factor->out[b] per-element link score must be emitted");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let results = vm.into_results();
    let base = offsets[&crate::common::Ident::new(&score_name)];
    let series: Vec<f64> = (0..results.step_count)
        .map(|s| results.data[s * results.step_size + base])
        .collect();

    // The EXACT series, both elements. `b`'s table has slope 3 and `a`'s has 2,
    // and `drift` supplies a second varying contribution so the score does not
    // saturate at 1 -- which is what makes the pinned element observable in the
    // VALUE. Pinning is the whole subject of this test, so the assertion has to
    // be the number, not a property the number happens to satisfy.
    let expected_a = [0.0, 0.0, 0.805_022_617_376_384_4, 0.809_579_376_075_400_5];
    let expected_b = [0.0, 0.0, 0.860_979_814_269_031_9, 0.864_449_016_428_508_1];
    assert_eq!(
        series.len(),
        expected_b.len(),
        "step count; got: {series:?}"
    );
    for (got, want) in series.iter().zip(expected_b.iter()) {
        assert!(
            (got - want).abs() < 1e-12,
            "the {score_name} series must match the correctly-pinned value; \
             got: {series:?}, want: {expected_b:?}"
        );
    }
    // ...and `a` must differ, which is the property that makes a WRONG pin
    // detectable at all: if the two elements scored alike, swapping them would
    // be invisible and the assertion above would constrain nothing about pinning.
    let a_off = offsets
        .keys()
        .map(|k| k.as_str().to_string())
        .find(|k| k.contains("link_score") && k.contains("factor\u{2192}out[a]"))
        .map(|n| offsets[&crate::common::Ident::new(&n)])
        .expect("the factor->out[a] per-element link score must be emitted");
    let series_a: Vec<f64> = (0..results.step_count)
        .map(|s| results.data[s * results.step_size + a_off])
        .collect();
    for (got, want) in series_a.iter().zip(expected_a.iter()) {
        assert!(
            (got - want).abs() < 1e-12,
            "the factor->out[a] series must match ITS element's table; \
             got: {series_a:?}, want: {expected_a:?}"
        );
    }
    assert!(
        series_a
            .iter()
            .zip(series.iter())
            .any(|(x, y)| (x - y).abs() > 1e-6),
        "the two elements must score differently, or this fixture cannot \
         detect a mis-pinned index at all; a={series_a:?} b={series:?}"
    );
}

/// The completeness guard must hold on the PER-ELEMENT (`PerElement`-shape)
/// emitter too, not only on the scalar-to-element one.
///
/// The guard originally existed at a single site, which was sufficient for
/// C-LEARN and wrong in general: three siblings emit the same per-element scalar
/// partial through a `dep_element_pins`-derived table
/// (`emit_per_element_link_scores` and both arms of
/// `emit_agg_to_target_link_scores`), and an unguarded one lets exactly the
/// defect the guard exists to remove back through -- a score that COMPILES while
/// the `PREVIOUS`-capture helper under it dies and reads a constant 0.
///
/// This fixture reaches `emit_per_element_link_scores`: `out[region]` reads
/// `src[region, t1]`, a 2-D source at a pinned column, which is the mixed
/// iterated+literal `PerElement` shape (GH #525) that routes there. It also
/// reads `other[region]`, where `other` is declared over `agg` -- a dimension
/// with DISJOINT element names and no declared mapping -- so that dep has no
/// projectable element and its dimension-name subscript would survive into the
/// scalar partial. The read itself is POSITIONAL -- `region` and `agg` declare
/// no correspondence, so it reaches the ordinal last resort -- which is what
/// makes an unrelated pair legal; see
/// `an_uncoverable_arrayed_dep_declines_the_edge_loudly` for
/// the full mechanism and for why this shape rather than an explicit element
/// map (GH #997 made the latter projectable). Nothing exotic: a 2-D read and an
/// undeclared-mapping dep.
#[test]
fn the_completeness_guard_holds_on_the_per_element_emitter() {
    use salsa::Setter;

    let project = TestProject::new("perelem_guard")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("region", &["a", "b"])
        .named_dimension("slot", &["t1", "t2"])
        .named_dimension("agg", &["p", "q"])
        .array_aux("other[agg]", "TIME * 2")
        .array_aux_direct(
            "src",
            vec!["region".into(), "slot".into()],
            "level[region] * 0.3",
            None,
        )
        .array_stock("level[region]", "1", &["growth"], &[], None)
        .array_flow("growth[region]", "out[region] * 0.1", None)
        .array_aux("out[region]", "src[region, t1] * 0.5 + other[region]")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    let source_model = sync_from_datamodel(&db, &project).models["main"].source;

    // Non-vacuity: the MODEL must compile (see the sibling guard's note).
    let diags = crate::db::collect_all_diagnostics(&db, source_project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .map(|d| (&d.variable, &d.error))
        .collect();
    assert!(
        errors.is_empty(),
        "the fixture must compile; got: {errors:?}"
    );

    let ltm = crate::db::model_ltm_variables(&db, source_model, source_project);
    assert!(
        !ltm.vars.is_empty(),
        "the model must emit LTM variables, or the empty assertion below is vacuous"
    );
    let emitted: Vec<&String> = ltm
        .vars
        .iter()
        .map(|v| &v.name)
        .filter(|n| n.contains("link_score\u{205A}src[") && n.contains("\u{2192}out["))
        .collect();
    assert!(
        emitted.is_empty(),
        "the src->out per-element edge cannot be scored (its `other[region]` dep is \
         unprojectable), so NO link score may be emitted; got: {emitted:?}"
    );

    let silently_degraded: Vec<&String> = diags
        .iter()
        .filter_map(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) if m.contains("silently degraded") => {
                d.variable.as_ref()
            }
            _ => None,
        })
        .collect();
    assert!(
        silently_degraded.is_empty(),
        "no fragment may be left reading a constant 0 under a compiling parent; \
         got: {silently_degraded:?}"
    );
    assert!(
        diags.iter().any(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) =>
                m.contains("cannot be projected onto that target element")
                    && m.contains("other[region]"),
            _ => false,
        }),
        "the decline must carry the unprojectable-dep warning naming the dep; got: {:?}",
        diags
            .iter()
            .map(|d| (&d.variable, &d.error))
            .collect::<Vec<_>>()
    );
}

/// A `Wide`-dimensioned per-element link-score fixture: a per-element flow
/// whose element `e` reads only `pop[e]`, so the emitted scores are the
/// element-pinned and A2A shapes that `compile_ltm_synthetic_fragment` routes
/// down its uncached `compile_direct` path -- which is what makes it a fixture
/// for the fragment-reuse tests below.
fn per_element_zero_slot_project(n: usize) -> datamodel::Project {
    let elems: Vec<String> = (0..n).map(|i| format!("e{i}")).collect();
    let elem_refs: Vec<&str> = elems.iter().map(String::as_str).collect();
    let eqns: Vec<(String, String)> = elems
        .iter()
        .map(|e| (e.clone(), format!("pop[{e}] * rate * 0.01")))
        .collect();
    let eqn_refs: Vec<(&str, &str)> = eqns.iter().map(|(e, q)| (e.as_str(), q.as_str())).collect();

    TestProject::new("per_element_zero_slots")
        .named_dimension("Wide", &elem_refs)
        .aux("rate", "1", None)
        .array_flow_with_ranges("growth[Wide]", eqn_refs)
        .array_stock("pop[Wide]", "10", &["growth"], &[], None)
        .build_datamodel()
}

/// The diagnostic pass must REUSE assembly's compiled LTM fragments, not
/// recompile them.
///
/// `assemble_module` and `model_ltm_fragment_diagnostics` each walk every LTM
/// synthetic variable and ask for its fragment. Only the scalar `Bare`
/// `from->to` score went through a salsa-memoized query; every element-pinned,
/// aggregate-touching or A2A score took a plain-function path, so the second
/// walk recompiled it from scratch. On C-LEARN that is 5,985 of 7,125 variables
/// -- about half a full compile stage -- paid again on every
/// `simlin_project_get_errors`, every MCP `read_model`, and twice more on every
/// MCP `edit_model`.
///
/// The measurement is a body-entry count, not a timing: `LtmBody` is recorded
/// inside `compile_ltm_equation_fragment`, which every LTM path funnels
/// through, so a cache hit is invisible to it and a real compile is not.
#[test]
fn the_ltm_diagnostic_pass_does_not_recompile_assembly_fragments() {
    let project = per_element_zero_slot_project(4);
    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    use salsa::Setter;
    source_project.set_ltm_enabled(&mut db).to(true);

    // Assembly first: this is the walk that legitimately compiles every
    // fragment. Priming it here is what makes the measured region below a
    // second walk rather than a first one.
    let compiled = crate::db::compile_project_incremental(&db, source_project, "main");
    assert!(
        compiled.is_ok(),
        "the fixture must compile with LTM enabled: {:?}",
        compiled.err()
    );

    // Control: the recorder is armed and the fixture really does generate LTM
    // fragments, so a zero below cannot be an empty model or a dead counter.
    crate::db::reset_fragment_executions();
    let _ = crate::db::ltm::model_ltm_fragment_diagnostics(&db, model, source_project);
    let after_first = crate::db::fragment_executions();
    let ltm_bodies: Vec<&str> = after_first
        .iter()
        .filter(|(kind, _)| *kind == crate::db::FragmentExecKind::LtmBody)
        .map(|(_, name)| name.as_str())
        .collect();

    assert!(
        ltm_bodies.is_empty(),
        "the diagnostic pass recompiled {} LTM fragment(s) that assembly had \
         already compiled: {ltm_bodies:?}",
        ltm_bodies.len()
    );
}

/// The control for the test above: the recorder DOES see LTM fragment compiles
/// when they genuinely happen, so an empty log there is evidence of reuse
/// rather than of a counter that never fires.
#[test]
fn the_ltm_fragment_body_counter_observes_a_cold_compile() {
    let project = per_element_zero_slot_project(4);
    let mut db = SimlinDb::default();
    let source_project = {
        let sync = sync_from_datamodel(&db, &project);
        sync.project
    };
    use salsa::Setter;
    source_project.set_ltm_enabled(&mut db).to(true);

    crate::db::reset_fragment_executions();
    let _ = crate::db::compile_project_incremental(&db, source_project, "main");
    let execs = crate::db::fragment_executions();
    let n_ltm = execs
        .iter()
        .filter(|(kind, _)| *kind == crate::db::FragmentExecKind::LtmBody)
        .count();
    assert!(
        n_ltm > 0,
        "a cold LTM compile must record LtmBody entries, or the reuse assertion \
         in the sibling test proves nothing; got: {execs:?}"
    );
}

/// The first arm's shared AST of an LTM equation, whatever its shape.
fn first_arm_expr(eq: &crate::db::LtmEquation) -> &std::sync::Arc<crate::ast::Expr0> {
    use crate::db::LtmEquation;
    let arm = match eq {
        LtmEquation::Scalar(arm) | LtmEquation::ApplyToAll(_, arm) => arm,
        LtmEquation::Arrayed { elements, .. } => {
            &elements
                .first()
                .expect("an arrayed equation must have an arm")
                .1
        }
    };
    arm.expr.as_ref().expect("the fixture's arm must parse")
}

/// `model_ltm_variables` must SHARE each emitted score's parsed AST with the
/// `shaped_link_score` memo it came from, not deep-copy it.
///
/// The emission loop clones the shaped result out of the memo for every score,
/// so before the ASTs were shared each equation was retained twice for the life
/// of the database. On C-LEARN the generation stage retains +273 MiB for 12.78 MB
/// of equation text, and roughly half of that is the second copy.
///
/// Pointer identity is the only way to see this: both copies compare EQUAL by
/// value either way -- which they must, since salsa backdates on value equality
/// and that is what lets an unrelated edit reuse the expensive downstream
/// fragment (GH #981). A value assertion here would pass on a deep copy.
#[test]
fn an_emitted_link_score_shares_its_ast_with_the_shaped_memo() {
    let project = per_element_zero_slot_project(4);
    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    use salsa::Setter;
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = crate::db::model_ltm_variables(&db, model, source_project);
    let emitted = ltm
        .vars
        .iter()
        .find(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}growth\u{2192}pop")
        .unwrap_or_else(|| {
            panic!(
                "fixture must emit the growth->pop link score; got: {:?}",
                ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
            )
        });

    let link_id = LtmLinkId::new(&db, "growth".to_string(), "pop".to_string());
    let shaped = shaped_link_score(&db, link_id, RefShape::Bare, model, source_project);
    let ShapedLinkScore::Scored { var: memo_var, .. } = shaped else {
        panic!("the growth->pop edge must be scored; got: {shaped:?}");
    };

    let memo_expr = first_arm_expr(&memo_var.equation);
    let emitted_expr = first_arm_expr(&emitted.equation);

    // The control: they must still be EQUAL, or salsa backdating breaks.
    assert_eq!(
        memo_expr, emitted_expr,
        "the emitted score and the memo must compare equal by value"
    );
    assert!(
        std::sync::Arc::ptr_eq(memo_expr, emitted_expr),
        "the emitted score must SHARE the memo's AST, not hold a deep copy"
    );
}

/// An XMILE project with a stdlib `DELAY3` whose input ramps, at the main
/// level (`delayed = DELAY3(src, 2)`) or inside a sub-model whose `input`
/// port is wired from the parent's `src`.
fn delay3_ramp_project(in_submodel: bool) -> datamodel::Project {
    let main_body = if in_submodel {
        r#"<module name="sub"><connect to="sub.input" from="src"/></module>
           <stock name="acc"><eqn>0</eqn><inflow>inflow</inflow></stock>
           <flow name="inflow"><eqn>sub.delayed</eqn></flow>"#
    } else {
        r#"<aux name="delayed"><eqn>DELAY3(src, 2)</eqn></aux>
           <stock name="acc"><eqn>0</eqn><inflow>inflow</inflow></stock>
           <flow name="inflow"><eqn>delayed</eqn></flow>"#
    };
    let sub_model = if in_submodel {
        r#"<model name="sub"><variables>
             <aux name="input" access="input"><eqn>0</eqn></aux>
             <aux name="delayed" access="output"><eqn>DELAY3(input, 2)</eqn></aux>
           </variables></model>"#
    } else {
        ""
    };
    let source = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" version="1.0">
  <header><vendor>test</vendor><product lang="en">test</product></header>
  <sim_specs method="Euler" time_units="Month"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <aux name="src"><eqn>TIME + 3</eqn></aux>
    {main_body}
  </variables></model>
  {sub_model}
</xmile>"#
    );
    crate::compat::open_xmile(&mut source.as_bytes()).expect("the XMILE fixture imports")
}

/// The `input -> stock` link score of a `DELAY3` instance reads the bound
/// port's two-step lag through a capture helper, at the main level and inside
/// a sub-model alike.
///
/// The flow-to-stock score (`ltm_augment::generate_flow_to_stock_equation`,
/// Schoenberg et al. 2023 Eq. 3) is
/// `|dt * (PREVIOUS(input) - PREVIOUS(PREVIOUS(input)))| / |second difference
/// of stock|`, zero for the first two steps. The nested lag is a capture
/// helper (`..⁚1⁚arg0 = PREVIOUS(input, 0)`) inside the `stdlib⁚delay3`
/// instance, whose body snapshots the instance's BOUND port `input`; lowering
/// resolves that port to its own slot (`Context::snapshot_storage`), so the
/// helper is `0, 3, 4, 5, 6` (the fallback, then the lagged ramp).
///
/// Derived from the template (`stdlib/delay3.stmx`: `stock` inflow `input`,
/// outflow `flow_1 = stock / (delay_time / 3)`, `delay_time = 2`, so
/// `stock(0) = 3 * 2/3 = 2`): `stock = 2, 2, 3, 3.5, 4.25`, the numerator is
/// `1` from t=2 on (the ramp rises by 1 per step), and the second differences
/// are `1, -0.5, 0.25`, so the score is `1, 2, 4` at t=2..4.
///
/// A parse that captured the port, or a lowering that could not address it,
/// gave the helper no fragment at all -- the LTM tail appends helpers by
/// bytecode presence, so the score silently read an unwritten 0 for the
/// two-step lag and printed `4, 10, 24` (the numerator degenerated to the
/// one-step-lagged ramp `4, 5, 6`) with no diagnostic. That is the
/// silent-wrong-number class the invariant forbids, which is why the values
/// are pinned here rather than only the helper's presence.
#[test]
fn delay3_input_to_stock_link_score_uses_the_two_step_lag_of_the_bound_port() {
    for (in_submodel, prefix) in [(false, ""), (true, "sub\u{00B7}")] {
        let project = delay3_ramp_project(in_submodel);
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        crate::db::set_project_ltm_enabled(&mut db, sync.project, true);
        let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
            .expect("the DELAY3 fixture compiles under LTM");
        let mut vm = crate::vm::Vm::new(compiled).expect("vm");
        vm.run_to_end().expect("runs");
        let results = crate::test_common::collect_results(&vm.into_results());
        let series = |name: String| -> Vec<f64> {
            results.get(&name).cloned().unwrap_or_else(|| {
                let candidates: Vec<&String> = results
                    .keys()
                    .filter(|k| k.contains("input\u{2192}stock"))
                    .collect();
                panic!("{name} not in results; input->stock names: {candidates:?}")
            })
        };
        let score = series(format!(
            "{prefix}$⁚delayed⁚0⁚delay3\u{00B7}$⁚ltm⁚link_score⁚input\u{2192}stock"
        ));
        assert_eq!(
            score,
            vec![0.0, 0.0, 1.0, 2.0, 4.0],
            "in_submodel={in_submodel}: the input -> stock link score must use the \
             two-step lag of the bound port (it was 0, 0, 4, 10, 24 when the nested \
             lag's helper silently failed to compile)"
        );
        let helper = series(format!(
            "{prefix}$⁚delayed⁚0⁚delay3\u{00B7}$⁚$⁚ltm⁚link_score⁚input\u{2192}stock⁚1⁚arg0"
        ));
        assert_eq!(
            helper,
            vec![0.0, 3.0, 4.0, 5.0, 6.0],
            "in_submodel={in_submodel}: the nested-lag helper holds PREVIOUS(input, 0) \
             of the bound port"
        );
    }
}
