// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the annealing OBJECTIVE: the cost the crossing-reduction search
//! minimizes. The objective must charge for every layout defect the search
//! could otherwise create while removing crossings -- piled-up nodes, labels
//! shoved into each other -- because the search will exploit any defect its
//! cost cannot see.

use super::*;

/// A relative footprint rect like `point_node_footprints` builds: a node shape
/// of radius 9 plus a label box of the given half-width hanging below it.
fn footprint(label_half_width: f64, label_height: f64) -> crate::diagram::common::Rect {
    crate::diagram::common::Rect {
        left: -label_half_width.max(9.0),
        right: label_half_width.max(9.0),
        top: -9.0,
        bottom: 9.0 + label_height,
    }
}

#[test]
fn test_footprint_overlap_zero_when_apart() {
    let mut layout: Layout<String> = BTreeMap::new();
    layout.insert("a".to_string(), Position::new(0.0, 0.0));
    layout.insert("b".to_string(), Position::new(500.0, 0.0));

    let mut footprints = HashMap::new();
    footprints.insert("a".to_string(), footprint(50.0, 14.0));
    footprints.insert("b".to_string(), footprint(50.0, 14.0));

    assert_eq!(
        point_node_footprint_overlap(&layout, &footprints),
        0.0,
        "far-apart footprints must cost nothing"
    );
}

#[test]
fn test_footprint_overlap_charges_label_collisions() {
    // Two nodes 60 apart: far enough that the old 50-unit pile-up floor sees
    // nothing, but their 50-half-width labels overlap by 40 units. The
    // footprint penalty must charge this.
    let mut layout: Layout<String> = BTreeMap::new();
    layout.insert("a".to_string(), Position::new(0.0, 0.0));
    layout.insert("b".to_string(), Position::new(60.0, 0.0));

    let mut footprints = HashMap::new();
    footprints.insert("a".to_string(), footprint(50.0, 14.0));
    footprints.insert("b".to_string(), footprint(50.0, 14.0));

    let cost = point_node_footprint_overlap(&layout, &footprints);
    assert!(
        cost > 0.0,
        "overlapping label footprints must have positive cost, got {cost}"
    );

    // Fully stacked nodes must cost at least a full unit (>= one crossing).
    let mut stacked: Layout<String> = BTreeMap::new();
    stacked.insert("a".to_string(), Position::new(0.0, 0.0));
    stacked.insert("b".to_string(), Position::new(0.0, 0.0));
    let stacked_cost = point_node_footprint_overlap(&stacked, &footprints);
    assert!(
        stacked_cost >= 1.0 - 1e-9,
        "fully stacked footprints must cost a full unit, got {stacked_cost}"
    );
    // And the gradient points the right way: closer costs more.
    assert!(
        stacked_cost > cost,
        "stacked ({stacked_cost}) must cost more than partially overlapping ({cost})"
    );
}

#[test]
fn test_footprint_overlap_ignores_nodes_missing_from_layout() {
    // Footprints for nodes not present in the layout are skipped, not a panic.
    let mut layout: Layout<String> = BTreeMap::new();
    layout.insert("a".to_string(), Position::new(0.0, 0.0));

    let mut footprints = HashMap::new();
    footprints.insert("a".to_string(), footprint(50.0, 14.0));
    footprints.insert("ghost".to_string(), footprint(50.0, 14.0));

    assert_eq!(point_node_footprint_overlap(&layout, &footprints), 0.0);
}

#[test]
fn test_footprint_overlap_is_deterministic() {
    // Float summation is order-dependent; the implementation must iterate in
    // sorted order so the result is bit-identical regardless of HashMap
    // insertion order (per-seed layout determinism, #633).
    let positions = [
        ("n1", 0.0, 0.0),
        ("n2", 30.0, 10.0),
        ("n3", 55.0, -5.0),
        ("n4", 80.0, 12.0),
    ];

    let build = |order: &[usize]| -> f64 {
        let mut layout: Layout<String> = BTreeMap::new();
        let mut footprints = HashMap::new();
        for &i in order {
            let (name, x, y) = positions[i];
            layout.insert(name.to_string(), Position::new(x, y));
            footprints.insert(name.to_string(), footprint(40.0, 14.0));
        }
        point_node_footprint_overlap(&layout, &footprints)
    };

    let forward = build(&[0, 1, 2, 3]);
    let reverse = build(&[3, 2, 1, 0]);
    assert!(
        forward.to_bits() == reverse.to_bits(),
        "footprint overlap must be bit-identical regardless of insertion order: \
         {forward} vs {reverse}"
    );
    assert!(forward > 0.0, "fixture nodes are close enough to overlap");
}
