// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Isolated-variable parking tests: a variable with no dependency edges and no
//! chain position takes no part in the force simulation and is parked in a
//! tidy, label-aware row below the diagram (`build_full_graph` /
//! `park_isolated_nodes`). Split out of `layout_tests.rs` to keep that file
//! under the per-file line cap, mirroring the `layout_selection_tests.rs`
//! precedent.

use std::collections::HashMap;

use super::*;
use crate::layout::config::LayoutConfig;
use crate::layout::metrics::compute_layout_metrics;
use crate::test_common::TestProject;

/// `TestProject::build_datamodel` synthesizes a single model named `"main"`.
const MAIN_MODEL: &str = "main";

/// A connected core: one stock-flow chain plus the two rate auxes that feed it.
fn connected_core() -> TestProject {
    TestProject::new("parking")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * death_rate", None)
        .aux("birth_rate", "0.03", None)
        .aux("death_rate", "0.01", None)
}

/// The names of the isolated constants `with_isolated_constants` adds: nothing
/// references them and they reference nothing -- the unused-parameter /
/// scenario-knob case common in real models (e.g. beer_game's noise and step
/// constants).
const ISOLATED_NAMES: [&str; 3] = ["unused_knob_a", "unused_knob_b", "unused_knob_c"];

/// The connected core plus three isolated constants.
fn with_isolated_constants() -> datamodel::Project {
    let mut project = connected_core();
    for name in ISOLATED_NAMES {
        project = project.aux(name, "42", None);
    }
    project.build_datamodel()
}

/// Map a laid-out view's aux/stock/flow elements to `(canonical name -> (x, y))`.
fn positions_by_name(view: &datamodel::StockFlow) -> HashMap<String, (f64, f64)> {
    view.elements
        .iter()
        .filter_map(|elem| match elem {
            ViewElement::Aux(a) => Some((canonicalize(&a.name).into_owned(), (a.x, a.y))),
            ViewElement::Stock(s) => Some((canonicalize(&s.name).into_owned(), (s.x, s.y))),
            ViewElement::Flow(f) => Some((canonicalize(&f.name).into_owned(), (f.x, f.y))),
            _ => None,
        })
        .collect()
}

/// Isolated variables must be parked in a tidy area below the connected
/// diagram -- not scattered by the force simulation. This mirrors how human
/// modelers handle unused/exogenous constants (e.g. the parked parameter rows
/// in the beer-game reference view).
#[test]
fn test_isolated_constants_are_parked_below_diagram() {
    let project = with_isolated_constants();
    let result = generate_layout(&project, MAIN_MODEL, None).unwrap();

    let positions = positions_by_name(&result);
    let isolated: Vec<(f64, f64)> = ISOLATED_NAMES.iter().map(|name| positions[*name]).collect();
    let connected: Vec<(f64, f64)> = positions
        .iter()
        .filter(|(name, _)| !ISOLATED_NAMES.contains(&name.as_str()))
        .map(|(_, pos)| *pos)
        .collect();
    assert_eq!(isolated.len(), 3, "all isolated constants must be laid out");
    assert!(!connected.is_empty());

    let conn_max_y = connected.iter().map(|(_, y)| *y).fold(f64::MIN, f64::max);
    let conn_min_x = connected.iter().map(|(x, _)| *x).fold(f64::MAX, f64::min);
    let conn_max_x = connected.iter().map(|(x, _)| *x).fold(f64::MIN, f64::max);

    let config = LayoutConfig::default();
    for &(x, y) in &isolated {
        // Parked BELOW everything that is connected...
        assert!(
            y > conn_max_y,
            "isolated constant at ({x}, {y}) should be below the connected diagram \
             (max y {conn_max_y})"
        );
        // ...but only just below: within two lane-heights, not flung away.
        assert!(
            y < conn_max_y + 2.0 * config.vertical_spacing,
            "isolated constant at ({x}, {y}) should be parked near the diagram \
             (max y {conn_max_y}), not flung"
        );
        // And horizontally in reading position within (or just past) the
        // diagram's own footprint -- not off in a far corner.
        assert!(
            x >= conn_min_x - config.horizontal_spacing
                && x <= conn_max_x + 8.0 * config.horizontal_spacing,
            "isolated constant at ({x}, {y}) should sit within the diagram's \
             horizontal footprint [{conn_min_x}, {conn_max_x}]"
        );
    }

    // Parked elements are tidily spaced: no two overlap.
    for i in 0..isolated.len() {
        for j in (i + 1)..isolated.len() {
            let dx = isolated[i].0 - isolated[j].0;
            let dy = isolated[i].1 - isolated[j].1;
            let dist = (dx * dx + dy * dy).sqrt();
            assert!(
                dist >= config.aux_width * 2.0,
                "parked constants {i} and {j} overlap: distance {dist}"
            );
        }
    }
}

/// Parked isolated constants must not overlap each other's labels, even with
/// long variable names (covid19's data variables have names like
/// "percent of total deaths from specified other disease"). The parking
/// spacing must be label-aware, not a fixed pitch.
#[test]
fn test_parked_constants_with_long_names_do_not_overlap_labels() {
    let long_names = [
        "percent reduction in deaths from specified other disease",
        "cumulative reported deaths from specified other condition",
        "fraction of population reporting reduced access to care",
        "average delay before deaths are reported to authorities",
    ];
    let mut project = TestProject::new("long_parking");
    for name in long_names {
        project = project.aux(name, "1", None);
    }
    let project = project.build_datamodel();
    let result = generate_layout(&project, MAIN_MODEL, None).unwrap();

    assert_eq!(
        result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Aux(_)))
            .count(),
        long_names.len()
    );

    // Every element in this view is a parked isolated constant, so ANY label
    // overlap the quality metric reports comes from the parking spacing.
    let metrics = compute_layout_metrics(&result, &LayoutConfig::default());
    assert_eq!(
        metrics.label_overlap, 0.0,
        "parked constants' labels must not overlap (label-aware spacing); \
         label_overlap = {}",
        metrics.label_overlap
    );
    assert_eq!(
        metrics.node_overlap, 0.0,
        "parked constants' shapes must not overlap; node_overlap = {}",
        metrics.node_overlap
    );
}

/// Adding isolated variables to a model must not move the connected part of
/// the layout AT ALL: isolated variables take no part in the force simulation
/// (an edge-less node only ever repels, so it both distorts its neighbors and
/// -- under any properly-converging force scheme -- gets flung unboundedly).
#[test]
fn test_isolated_constants_do_not_perturb_connected_layout() {
    let base_project = connected_core().build_datamodel();
    let base = generate_layout(&base_project, MAIN_MODEL, None).unwrap();

    let with_isolated_project = with_isolated_constants();
    let with_isolated = generate_layout(&with_isolated_project, MAIN_MODEL, None).unwrap();

    let base_positions = positions_by_name(&base);
    let with_positions = positions_by_name(&with_isolated);

    for (name, (bx, by)) in &base_positions {
        if ISOLATED_NAMES.contains(&name.as_str()) {
            continue;
        }
        let (wx, wy) = with_positions
            .get(name)
            .unwrap_or_else(|| panic!("connected element {name} missing"));
        assert!(
            (bx - wx).abs() < 1e-6 && (by - wy).abs() < 1e-6,
            "connected element {name} moved when isolated constants were added: \
             ({bx}, {by}) -> ({wx}, {wy})"
        );
    }
}
