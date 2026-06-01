// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Rung-0 layout-selection and regression-guard tests (Phase 5 of the layout
//! quality eval): `select_best_layout` picks the lowest `weighted_cost`
//! candidate (even when that means *more* connector crossings than a rival),
//! the deterministic per-model `weighted_cost` ceiling guards against quality
//! regressions, and a fixed seed reproduces a byte-identical layout. Split out
//! of `layout_tests.rs` to keep that file under the per-file line cap, mirroring
//! the `crossings_tests.rs` precedent.

use super::*;
use crate::datamodel;
use crate::layout::metrics::{MetricWeights, compute_layout_metrics};
use crate::test_common::TestProject;

/// `TestProject::build_datamodel` synthesizes a single model named `"main"`, so
/// every `generate_layout_with_config` call in this file targets that name.
const MAIN_MODEL: &str = "main";

/// A scalar aux at (`x`, `y`) with a unique name, so a selected view can be
/// identified by which marker element it carries.
fn marker_aux(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn sel_link(uid: i32, from_uid: i32, to_uid: i32) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape: LinkShape::Straight,
        polarity: None,
    })
}

/// Wrap a set of view elements into a `StockFlow` carrying `name` as its marker
/// so `select_best_layout`'s winner is identifiable.
fn sel_view(name: &str, elements: Vec<ViewElement>) -> datamodel::StockFlow {
    datamodel::StockFlow {
        name: Some(name.to_string()),
        elements,
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 1000.0,
        },
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    }
}

/// A view whose two straight links cross exactly once (the diagonals of a
/// square): `count_view_crossings == 1`.
fn crossing_view(name: &str) -> datamodel::StockFlow {
    sel_view(
        name,
        vec![
            marker_aux(1, "a1", 0.0, 0.0),
            marker_aux(2, "a2", 100.0, 100.0),
            marker_aux(3, "a3", 0.0, 100.0),
            marker_aux(4, "a4", 100.0, 0.0),
            sel_link(10, 1, 2),
            sel_link(11, 3, 4),
        ],
    )
}

/// A view whose two straight links share an endpoint and never cross:
/// `count_view_crossings == 0`.
fn non_crossing_view(name: &str) -> datamodel::StockFlow {
    sel_view(
        name,
        vec![
            marker_aux(1, "a1", 50.0, 50.0),
            marker_aux(2, "a2", 100.0, 0.0),
            marker_aux(3, "a3", 100.0, 100.0),
            sel_link(10, 1, 2),
            sel_link(11, 1, 3),
        ],
    )
}

/// AC6.1: selection minimizes `weighted_cost`, not crossings. The lowest-cost
/// candidate is deliberately built from a view with MORE connector crossings
/// than a rival, so the old "fewest crossings" rule would have picked the other
/// one. We assert the crossing inversion is real (via `count_view_crossings`),
/// then assert `select_best_layout` returns the lowest-`weighted_cost` view.
#[test]
fn test_select_best_layout_minimizes_weighted_cost_over_crossings() {
    let crossing = crossing_view("more_crossings_low_cost");
    let non_crossing = non_crossing_view("fewer_crossings_high_cost");

    // The inversion is genuine, not just narrative: the candidate we expect to
    // win actually has strictly more crossings than the one we expect to lose.
    let crossing_count = count_view_crossings(&crossing);
    let non_crossing_count = count_view_crossings(&non_crossing);
    assert_eq!(crossing_count, 1, "crossing view should have one crossing");
    assert_eq!(
        non_crossing_count, 0,
        "non-crossing view should have zero crossings"
    );
    assert!(
        crossing_count > non_crossing_count,
        "the low-cost candidate must have more crossings than its rival, \
         so the choice differs from the old crossings-only rule"
    );

    // Hand-set costs so the MORE-crossings view is the cheaper one. Under the
    // retired crossings-only rule `fewer_crossings_high_cost` (0 crossings)
    // would win; under Rung 0 the lower `weighted_cost` wins.
    let results = vec![
        Ok(LayoutResult {
            view: crossing,
            weighted_cost: 1.0,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: non_crossing,
            weighted_cost: 5.0,
            seed: 123,
        }),
    ];

    let best = select_best_layout(results).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("more_crossings_low_cost"),
        "the lowest-weighted_cost candidate must win even with more crossings"
    );
}

/// AC6.1 (tie-break): equal `weighted_cost`, the lower seed wins. This is the
/// same rule `test_select_best_layout_lowest_seed_on_tie` (in `layout_tests.rs`)
/// pins on hand-built `StockFlow` literals; here we re-assert it through the
/// marker-named helpers for completeness alongside the cost-ordering case.
#[test]
fn test_select_best_layout_tie_breaks_on_lowest_seed() {
    let results = vec![
        Ok(LayoutResult {
            view: sel_view("seed_456", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 2.5,
            seed: 456,
        }),
        Ok(LayoutResult {
            view: sel_view("seed_42", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 2.5,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: sel_view("seed_789", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 2.5,
            seed: 789,
        }),
    ];

    let best = select_best_layout(results).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("seed_42"),
        "on a weighted_cost tie the lowest seed wins"
    );
}

/// AC6.1 (NaN safety): a NaN-cost challenger must never displace a finite
/// running best. `select_best_layout` keeps the running best whenever the
/// challenger's `<` comparison is false, and `challenger < finite` is always
/// false for a NaN challenger -- so a degenerate NaN-cost candidate encountered
/// after a finite one cannot win.
#[test]
fn test_select_best_layout_nan_challenger_never_displaces_finite() {
    // Finite candidate first, then NaN: the NaN must not displace it.
    let finite_first = vec![
        Ok(LayoutResult {
            view: sel_view("finite", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 4.0,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: sel_view("nan", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: f64::NAN,
            seed: 123,
        }),
    ];
    let best = select_best_layout(finite_first).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("finite"),
        "a NaN-cost challenger must not displace a finite running best"
    );

    // A NaN that arrives last among several finite candidates still loses: the
    // finite minimum is already the running best by the time NaN is compared.
    let nan_last = vec![
        Ok(LayoutResult {
            view: sel_view("hi", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 9.0,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: sel_view("lo", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 1.0,
            seed: 123,
        }),
        Ok(LayoutResult {
            view: sel_view("nan", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: f64::NAN,
            seed: 456,
        }),
    ];
    let best = select_best_layout(nan_last).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("lo"),
        "the finite minimum wins; a trailing NaN candidate cannot displace it"
    );
}

/// AC6.1 (NaN safety, order-independent): a finite challenger must beat a NaN
/// running best regardless of position. The fold seeds the running best with the
/// FIRST result, so a degenerate NaN-cost layout from the first seed could
/// otherwise become a sticky running best (`finite < NaN` is false and `finite
/// == NaN` is false, so a plain `<` comparison never overtakes it). The fold
/// special-cases a NaN running best so a later finite candidate always wins. In
/// production (`generate_best_layout` runs seeds in the fixed order [42, 123,
/// 456, 789]), this guarantees a usable finite layout is shipped whenever ANY
/// seed produced one, no matter which seed degenerated.
#[test]
fn test_select_best_layout_finite_beats_nan_running_best() {
    let nan_first = vec![
        Ok(LayoutResult {
            view: sel_view("nan", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: f64::NAN,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: sel_view("finite", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: 4.0,
            seed: 123,
        }),
    ];
    let best = select_best_layout(nan_first).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("finite"),
        "a finite challenger must beat a NaN running best regardless of order"
    );
}

/// AC6.1 (NaN safety, all-NaN determinism): when EVERY candidate has a NaN cost,
/// neither the `<` comparison nor the NaN special-cases fire (a NaN challenger is
/// never "better"), so the earliest candidate is kept. This is deterministic
/// regardless of seed order -- the production caller would ship the first seed's
/// (degenerate) layout, but the choice is reproducible rather than arbitrary.
#[test]
fn test_select_best_layout_all_nan_keeps_earliest() {
    let all_nan = vec![
        Ok(LayoutResult {
            view: sel_view("first", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: f64::NAN,
            seed: 456,
        }),
        Ok(LayoutResult {
            view: sel_view("second", vec![marker_aux(1, "a", 0.0, 0.0)]),
            weighted_cost: f64::NAN,
            seed: 42,
        }),
    ];
    let best = select_best_layout(all_nan).expect("selection should succeed");
    assert_eq!(
        best.name.as_deref(),
        Some("first"),
        "when all candidates are NaN the earliest is kept deterministically"
    );
}

// ---- AC7: deterministic weighted_cost regression guard ----
//
// The thresholds below are observed-cost CEILINGS captured at the fixed
// annealing seed 42 with the calibrated `MetricWeights::default()`. They guard
// against layout-quality regressions: if a change to the layout algorithm,
// metric, or weights pushes a tiny model's fixed-seed `weighted_cost` above its
// ceiling, this test fails loudly. Each ceiling sits a small margin above the
// observed cost (roughly observed * 1.15, or a small absolute floor when the
// observed cost is 0) -- tight enough to catch a real regression, loose enough
// not to flake on float noise.
//
// To regenerate after an INTENTIONAL metric/weight change: layout is
// deterministic per seed, so print the new `weighted_cost` for each guard model
// (e.g. add a temporary `println!` to `guard_fixed_seed_cost`), run this test
// once, and reset each ceiling a small margin above the new observed value.
// Lowering a ceiling that no longer matches reality is fine; raising one to
// paper over a real regression is not.
//
// Observed at seed 42 (2026-05-31), after the quiescence work and with the
// sprawl compactness counterweight (0.1) in MetricWeights::default():
// pop = 0.2023, chain = 0.4959, two_stock = 0.1373. (Each cost now includes
// ~0.1-0.15 of sprawl-times-weight; the readability terms themselves are near
// zero on these tiny models.) The regeneration procedure printed these via the
// GUARD_REGEN lines this test emits.
const GUARD_POP_COST_CEILING: f64 = 0.24;
const GUARD_CHAIN_COST_CEILING: f64 = 0.57;
const GUARD_TWO_STOCK_COST_CEILING: f64 = 0.16;

/// Lay `project`'s `main` model out at the fixed seed 42 and return its
/// calibrated `weighted_cost`. Seeding explicitly (rather than relying on the
/// `LayoutConfig::default()` seed) keeps the guard pinned to one reproducible
/// layout even if the default seed changes.
fn guard_fixed_seed_cost(project: &datamodel::Project) -> f64 {
    let config = LayoutConfig {
        annealing_random_seed: 42,
        ..LayoutConfig::default()
    };
    let view = generate_layout_with_config(project, MAIN_MODEL, config.clone(), None)
        .expect("layout generation should succeed");
    compute_layout_metrics(&view, &config).weighted_cost(&MetricWeights::default())
}

/// A population stock with births/deaths flows and two rate auxes -- the
/// canonical tiny feedback model.
fn guard_pop_model() -> datamodel::Project {
    TestProject::new("guard_pop")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * death_rate", None)
        .aux("birth_rate", "0.03", None)
        .aux("death_rate", "0.01", None)
        .build_datamodel()
}

/// A pure auxiliary dependency chain (no stocks): a -> b -> c -> d.
fn guard_chain_model() -> datamodel::Project {
    TestProject::new("guard_chain")
        .aux("a", "1", None)
        .aux("b", "a * 2", None)
        .aux("c", "b + a", None)
        .aux("d", "c * b", None)
        .build_datamodel()
}

/// A two-stock transfer model: source -> transfer -> sink, rate-driven.
fn guard_two_stock_model() -> datamodel::Project {
    TestProject::new("guard_two_stock")
        .stock("source", "100", &[], &["transfer"], None)
        .stock("sink", "0", &["transfer"], &[], None)
        .flow("transfer", "source * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel()
}

/// AC7.1: the fixed-seed `weighted_cost` of each tiny guard model stays at or
/// below its committed ceiling. Fast and deterministic: three tiny models, one
/// seed each.
#[test]
fn test_weighted_cost_regression_guard() {
    let cases: [(&str, datamodel::Project, f64); 3] = [
        ("pop", guard_pop_model(), GUARD_POP_COST_CEILING),
        ("chain", guard_chain_model(), GUARD_CHAIN_COST_CEILING),
        (
            "two_stock",
            guard_two_stock_model(),
            GUARD_TWO_STOCK_COST_CEILING,
        ),
    ];

    // Print every cost before asserting so a regeneration run sees all three.
    let costs: Vec<(&str, f64, f64)> = cases
        .iter()
        .map(|(name, project, ceiling)| (*name, guard_fixed_seed_cost(project), *ceiling))
        .collect();
    for (name, cost, _) in &costs {
        eprintln!("GUARD_REGEN: {name} = {cost}");
    }
    for (name, cost, ceiling) in costs {
        assert!(
            cost <= ceiling,
            "{name}: fixed-seed weighted_cost {cost} exceeded ceiling {ceiling} \
             -- a layout-quality regression (or an intentional metric/weight \
             change that needs the ceiling regenerated)"
        );
    }
}

/// AC7.2: the guard ceiling actually discriminates good layouts from bad ones.
/// We take a real fixed-seed layout of the pop model and pile every node onto
/// the same coordinate, blowing up the node-overlap term, then assert the
/// resulting `weighted_cost` exceeds the ceiling -- so a real layout that
/// regressed to this level WOULD trip `test_weighted_cost_regression_guard`.
/// This makes the failure direction explicit and testable without flakiness.
#[test]
fn test_weighted_cost_guard_rejects_degenerate_layout() {
    let project = guard_pop_model();
    let config = LayoutConfig {
        annealing_random_seed: 42,
        ..LayoutConfig::default()
    };
    let view = generate_layout_with_config(&project, MAIN_MODEL, config.clone(), None)
        .expect("layout generation should succeed");

    // Collapse every positioned node onto the origin so the shapes overlap
    // maximally (links/aliases/groups have no independent position).
    let mut degenerate = view.clone();
    for elem in &mut degenerate.elements {
        match elem {
            ViewElement::Aux(a) => {
                a.x = 0.0;
                a.y = 0.0;
            }
            ViewElement::Stock(s) => {
                s.x = 0.0;
                s.y = 0.0;
            }
            ViewElement::Flow(f) => {
                f.x = 0.0;
                f.y = 0.0;
            }
            ViewElement::Module(m) => {
                m.x = 0.0;
                m.y = 0.0;
            }
            ViewElement::Cloud(c) => {
                c.x = 0.0;
                c.y = 0.0;
            }
            ViewElement::Link(_) | ViewElement::Alias(_) | ViewElement::Group(_) => {}
        }
    }

    let degenerate_cost =
        compute_layout_metrics(&degenerate, &config).weighted_cost(&MetricWeights::default());
    assert!(
        degenerate_cost > GUARD_POP_COST_CEILING,
        "a degenerate all-overlapping layout (cost {degenerate_cost}) must exceed \
         the guard ceiling {GUARD_POP_COST_CEILING}, proving the guard discriminates"
    );
}

/// A model with enough nodes (a stock fed/drained by ten leaf auxes through two
/// flows) to exercise the seeded SFDP/annealing path while remaining small
/// enough for a fast determinism guard.
fn guard_seed_sensitive_model() -> datamodel::Project {
    let mut tp = TestProject::new("guard_seed_sensitive")
        .stock("s", "100", &["inflow"], &["outflow"], None)
        .flow("inflow", "a1 + a2 + a3 + a4 + a5", None)
        .flow("outflow", "b1 + b2 + b3 + b4 + b5", None);
    for i in 1..=5 {
        tp = tp.aux(&format!("a{i}"), "1", None);
        tp = tp.aux(&format!("b{i}"), "1", None);
    }
    tp.build_datamodel()
}

/// Lay `project`'s `main` model out at `seed`.
fn layout_at_seed(project: &datamodel::Project, seed: u64) -> datamodel::StockFlow {
    let config = LayoutConfig {
        annealing_random_seed: seed,
        ..LayoutConfig::default()
    };
    generate_layout_with_config(project, MAIN_MODEL, config, None)
        .expect("layout generation should succeed")
}

/// AC8.1: a fixed seed reproduces a byte-identical layout. Generating the same
/// model twice through `generate_layout_with_config` at the same explicit seed
/// must yield two `StockFlow` values that compare equal (`StockFlow` derives
/// `PartialEq`, so this checks every field -- positions, view box, element
/// order -- not just element counts).
///
/// This per-seed reproducibility is distinct from the Phase 3 M-seed
/// statistical sweep, which deliberately VARIES the seed to sample the layout
/// distribution. Here the seed is held fixed and the layout must be exactly
/// repeatable; there the seed sweeps and may produce the same result on models
/// where semantic initial placement already determines the stable layout.
/// The integration test `tests/layout.rs` already asserts `view1 == view2` for
/// `generate_layout`; this focused in-crate test covers the
/// `generate_layout_with_config` + explicit-seed Rung-0 path.
#[test]
fn test_layout_is_byte_identical_for_fixed_seed() {
    let project = guard_seed_sensitive_model();

    let view1 = layout_at_seed(&project, 7);
    let view2 = layout_at_seed(&project, 7);
    assert_eq!(
        view1, view2,
        "the same model at the same fixed seed must produce a byte-identical layout"
    );
}
