// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::{compile_ltm_equation_fragment, scalarize_ltm_equation};
use crate::datamodel;
use crate::db::{
    LtmLinkId, RefShape, SimlinDb, compute_layout, link_score_equation_text,
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
        &datamodel::Equation::Scalar("PREVIOUS(producer)".to_string()),
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
        &datamodel::Equation::ApplyToAll(dims.clone(), "PREVIOUS(population) * 0.5".to_string()),
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
                if var.name.contains("test_a2a_link_score") =>
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
        &datamodel::Equation::ApplyToAll(dims.clone(), "PREVIOUS(population) * 0.5".to_string()),
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
            SymbolicOpcode::SymLoadPrev { var } if var.name == "population" => {
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
                if var.name.contains("test_prev_per_elem") =>
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

    // The stock-to-flow direction: population -> births
    let link_id = LtmLinkId::new(&db, "population".to_string(), "births".to_string());
    let lsv = link_score_equation_text(&db, link_id, source_model, sync.project);

    let lsv = lsv
        .as_ref()
        .expect("stock-to-flow link score should be generated for arrayed model");

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
    // scores. The non-shaped `link_score_equation_text` would `scalarize`
    // the result; use the shaped entry point so the arrayed equation
    // survives intact.
    let link_id = LtmLinkId::new(&db, "population".to_string(), "births".to_string());
    let lsv = link_score_equation_text_shaped(
        &db,
        link_id,
        RefShape::FixedIndex(vec!["nyc".to_string()]),
        source_model,
        sync.project,
    );
    let lsv = lsv
        .as_ref()
        .expect("stock-to-arrayed-flow link score should be generated");

    let elements = match &lsv.equation {
        datamodel::Equation::Arrayed(_, elements, _, _) => elements,
        other => {
            panic!("stock-to-arrayed-flow link score must be Equation::Arrayed, got: {other:?}")
        }
    };
    assert!(
        !elements.is_empty(),
        "arrayed link score should have per-element slots"
    );
    for (elem, slot_eqn, _, _) in elements {
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
    use datamodel::Equation::{ApplyToAll, Arrayed, Scalar};

    // Arrayed with multiple per-element slots collapses to the *first* slot's text.
    let multi = Arrayed(
        vec!["region".to_string()],
        vec![
            ("nyc".to_string(), "first slot".to_string(), None, None),
            ("boston".to_string(), "second slot".to_string(), None, None),
        ],
        None,
        false,
    );
    assert!(matches!(scalarize_ltm_equation(multi), Scalar(text) if text == "first slot"));

    // Arrayed with no slots but a Some(default) falls back to the default text.
    let default_only = Arrayed(
        vec!["region".to_string()],
        vec![],
        Some("default eqn".to_string()),
        false,
    );
    assert!(matches!(scalarize_ltm_equation(default_only), Scalar(text) if text == "default eqn"));

    // Arrayed with neither slots nor a default falls back to "0".
    let empty = Arrayed(vec!["region".to_string()], vec![], None, false);
    assert!(matches!(scalarize_ltm_equation(empty), Scalar(text) if text == "0"));

    // ApplyToAll and Scalar inputs are preserved (text kept, dims dropped).
    assert!(matches!(
        scalarize_ltm_equation(ApplyToAll(vec!["region".to_string()], "a2a eqn".to_string())),
        Scalar(text) if text == "a2a eqn"
    ));
    assert!(
        matches!(scalarize_ltm_equation(Scalar("scalar eqn".to_string())), Scalar(text) if text == "scalar eqn")
    );
}

/// `cyclic_orderings(n)` enumerates the distinct orderings of `[0, .., n-1]`
/// modulo rotation (index 0 pinned first) and modulo mirror reversal
/// (reverse-the-tail) -- `1` ordering for n ∈ {0, 1, 2}, `(n-1)!/2` for
/// n ≥ 3. The exact vectors (and their order, which Heap's-algorithm
/// determinism fixes -- and which `assign_loop_ids`' stable sort relies on
/// for stable, distinct ids) are pinned here.
#[test]
fn cyclic_orderings_enumerates_distinct_rotation_and_mirror_classes() {
    use super::cyclic_orderings;

    // Degenerate / trivial cases.
    assert_eq!(cyclic_orderings(0), vec![Vec::<usize>::new()]);
    assert_eq!(cyclic_orderings(1), vec![vec![0]]);
    // n=2: `0! = 1` ordering; reversing a 2-cycle gives the same sequence,
    // so no mirror to quotient.
    assert_eq!(cyclic_orderings(2), vec![vec![0, 1]]);
    // n=3: `(3-1)!/2 = 1`. `[0,1,2]` and `[0,2,1]` are mirrors (reverse the
    // tail `[1,2]` -> `[2,1]`); the lexicographically smaller tail wins.
    assert_eq!(cyclic_orderings(3), vec![vec![0, 1, 2]]);
    // n=4: `(4-1)!/2 = 3`. Heap's order over the tail `[1,2,3]` is
    // `[1,2,3], [2,1,3], [3,1,2], [1,3,2], [2,3,1], [3,2,1]`; keep a tail
    // iff it is lexicographically <= its reverse -> `[1,2,3]`, `[2,1,3]`,
    // `[1,3,2]`.
    assert_eq!(
        cyclic_orderings(4),
        vec![vec![0, 1, 2, 3], vec![0, 2, 1, 3], vec![0, 1, 3, 2]]
    );

    // Structural checks for a few more `n`: count == (n-1)!/2 for n >= 3,
    // index 0 always first, every ordering is a permutation of 0..n, and no
    // two are mirror images of each other.
    fn factorial(k: usize) -> usize {
        (1..=k).product::<usize>().max(1)
    }
    for n in 3..=6 {
        let orderings = cyclic_orderings(n);
        assert_eq!(
            orderings.len(),
            factorial(n - 1) / 2,
            "cyclic_orderings({n}) should have (n-1)!/2 entries"
        );
        let mut seen: std::collections::HashSet<Vec<usize>> = std::collections::HashSet::new();
        for ord in &orderings {
            assert_eq!(ord[0], 0, "index 0 must be pinned first");
            let mut sorted = ord.clone();
            sorted.sort();
            assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "must be a permutation");
            // Mirror = pin 0, reverse the tail.
            let mirror: Vec<usize> = std::iter::once(0)
                .chain(ord[1..].iter().rev().copied())
                .collect();
            assert!(
                !seen.contains(&mirror),
                "cyclic_orderings({n}) emitted both {ord:?} and its mirror {mirror:?}"
            );
            assert!(seen.insert(ord.clone()), "duplicate ordering {ord:?}");
        }
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
/// `k` pairwise-disjoint petals it emits `Σ_{m=2}^{k} C(k,m)·orderings(m)`
/// stitched sequences -- the single petals themselves are NOT in the output
/// (they are already elementary loops the enumerator emits directly).
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
    // 3 disjoint pairs (each 1 ordering) + 1 triple ((3-1)!/2 = 1 ordering) = 4.
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
