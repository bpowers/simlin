// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f64::consts::PI;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{LabelSide, LinkShape};

use super::graph::Position;

const TWO_PI: f64 = 2.0 * PI;

/// Normalize an angle in radians to [0, 2*pi).
fn normalize_to_two_pi(angle: f64) -> f64 {
    let mut a = angle % TWO_PI;
    if a < 0.0 {
        a += TWO_PI;
    }
    a
}

/// Collect all connector angles around a variable, normalized to [0, 2*pi).
///
/// For outgoing connections (vars that depend on this one), the angle points
/// from this variable toward the dependent. For incoming connections (vars
/// this one depends on), the angle points from this variable back toward
/// the source (reversed direction), so that the angle represents the visual
/// direction the connector "leaves" from this node.
pub fn connector_angles(
    var_name: &str,
    positions: &HashMap<String, Position>,
    uses: &BTreeMap<String, BTreeSet<String>>,
    used_by: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<f64> {
    let Some(&pos) = positions.get(var_name) else {
        return Vec::new();
    };

    let mut angles = Vec::new();

    // Outgoing: vars that depend on this one
    if let Some(dependents) = used_by.get(var_name) {
        for dep in dependents {
            if let Some(&dep_pos) = positions.get(dep.as_str()) {
                let dx = dep_pos.x - pos.x;
                let dy = dep_pos.y - pos.y;
                angles.push(normalize_to_two_pi(dy.atan2(dx)));
            }
        }
    }

    // Incoming: vars this one depends on (reversed direction)
    if let Some(sources) = uses.get(var_name) {
        for src in sources {
            if let Some(&src_pos) = positions.get(src.as_str()) {
                let dx = pos.x - src_pos.x;
                let dy = pos.y - src_pos.y;
                angles.push(normalize_to_two_pi(dy.atan2(dx)));
            }
        }
    }

    angles
}

/// Find the midpoint angle of the largest gap between sorted angles in [0, 2*pi).
///
/// Returns 0 if there are no angles. For a single angle, returns the
/// opposite side (angle + pi, normalized).
pub fn mid_gap_angle(angles: &[f64]) -> f64 {
    if angles.is_empty() {
        return 0.0;
    }

    let mut sorted = angles.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));

    if sorted.len() == 1 {
        return normalize_to_two_pi(sorted[0] + PI);
    }

    let mut max_gap = 0.0_f64;
    let mut max_gap_start = 0.0_f64;

    for i in 0..sorted.len() - 1 {
        let gap = sorted[i + 1] - sorted[i];
        if gap > max_gap {
            max_gap = gap;
            max_gap_start = sorted[i];
        }
    }

    // Wrap-around gap
    let wrap_gap = (sorted[0] + TWO_PI) - sorted[sorted.len() - 1];
    if wrap_gap > max_gap {
        max_gap = wrap_gap;
        max_gap_start = sorted[sorted.len() - 1];
    }

    normalize_to_two_pi(max_gap_start + max_gap / 2.0)
}

/// Map mid-gap angle to the cardinal label side.
///
/// The angle ranges are chosen so that the label is placed in the direction
/// with the most free space around the variable.
pub fn calculate_optimal_label_side(
    var_name: &str,
    positions: &HashMap<String, Position>,
    uses: &BTreeMap<String, BTreeSet<String>>,
    used_by: &BTreeMap<String, BTreeSet<String>>,
) -> LabelSide {
    let angles = connector_angles(var_name, positions, uses, used_by);
    if angles.is_empty() {
        return LabelSide::Bottom;
    }

    let angle = mid_gap_angle(&angles);
    angle_to_label_side(angle)
}

fn angle_to_label_side(angle: f64) -> LabelSide {
    let a = normalize_to_two_pi(angle);
    let quarter = PI / 4.0;

    if a < quarter || a >= 7.0 * quarter {
        LabelSide::Right
    } else if a < 3.0 * quarter {
        LabelSide::Bottom
    } else if a < 5.0 * quarter {
        LabelSide::Left
    } else {
        LabelSide::Top
    }
}

/// Angle for each cardinal LabelSide, used in restricted placement scoring.
fn label_side_angle(side: LabelSide) -> f64 {
    match side {
        LabelSide::Right => 0.0,
        LabelSide::Bottom => PI / 2.0,
        LabelSide::Left => PI,
        LabelSide::Top => 3.0 * PI / 2.0,
        LabelSide::Center => PI / 2.0, // treat same as Bottom
    }
}

/// Minimum angular distance between two angles on a circle [0, 2*pi).
fn angular_distance(a: f64, b: f64) -> f64 {
    let diff = (a - b).abs() % TWO_PI;
    diff.min(TWO_PI - diff)
}

/// Calculate optimal label side constrained to the set of allowed sides.
///
/// When there are no connections, prefers Bottom > Right > Left > Top among
/// the allowed sides. Otherwise, scores each allowed side by the minimum
/// angular clearance from any connection angle, breaking ties by lowest
/// connection density in the side's angular band, then by the preference
/// order above.
pub fn calculate_restricted_label_side(
    var_name: &str,
    positions: &HashMap<String, Position>,
    uses: &BTreeMap<String, BTreeSet<String>>,
    used_by: &BTreeMap<String, BTreeSet<String>>,
    allowed: &[LabelSide],
) -> LabelSide {
    if allowed.is_empty() {
        return LabelSide::Bottom;
    }

    let angles = connector_angles(var_name, positions, uses, used_by);

    if angles.is_empty() {
        let preference = [
            LabelSide::Bottom,
            LabelSide::Right,
            LabelSide::Left,
            LabelSide::Top,
        ];
        for &side in &preference {
            if allowed.contains(&side) {
                return side;
            }
        }
        return allowed[0];
    }

    let band = PI / 8.0;

    let mut best_side = allowed[0];
    let mut best_clearance = f64::NEG_INFINITY;
    let mut best_density = usize::MAX;
    let mut best_preference = usize::MAX;

    let preference_order = [
        LabelSide::Bottom,
        LabelSide::Right,
        LabelSide::Left,
        LabelSide::Top,
    ];

    for &side in allowed {
        let side_angle = label_side_angle(side);

        let min_clearance = angles
            .iter()
            .map(|&a| angular_distance(side_angle, a))
            .fold(f64::INFINITY, f64::min);

        let density = angles
            .iter()
            .filter(|&&a| angular_distance(side_angle, a) < band)
            .count();

        let pref_idx = preference_order
            .iter()
            .position(|&s| s == side)
            .unwrap_or(usize::MAX);

        let is_better = min_clearance > best_clearance
            || (min_clearance == best_clearance && density < best_density)
            || (min_clearance == best_clearance
                && density == best_density
                && pref_idx < best_preference);

        if is_better {
            best_side = side;
            best_clearance = min_clearance;
            best_density = density;
            best_preference = pref_idx;
        }
    }

    best_side
}

/// Shift all element positions so that the minimum coordinates equal `margin`.
///
/// Scans every element type for coordinates (including FlowPoint coordinates
/// on Flows, control points on MultiPoint Links, and Cloud positions), then
/// applies a uniform translation.
pub fn normalize_coordinates(elements: &mut [ViewElement], margin: f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;

    for elem in elements.iter() {
        match elem {
            ViewElement::Aux(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
            ViewElement::Stock(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
            ViewElement::Flow(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
                for pt in &v.points {
                    min_x = min_x.min(pt.x);
                    min_y = min_y.min(pt.y);
                }
            }
            ViewElement::Link(v) => {
                if let LinkShape::MultiPoint(ref pts) = v.shape {
                    for pt in pts {
                        min_x = min_x.min(pt.x);
                        min_y = min_y.min(pt.y);
                    }
                }
            }
            ViewElement::Module(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
            ViewElement::Alias(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
            ViewElement::Cloud(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
            ViewElement::Group(v) => {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
            }
        }
    }

    if min_x == f64::INFINITY || min_y == f64::INFINITY {
        return;
    }

    let dx = margin - min_x;
    let dy = margin - min_y;

    if dx == 0.0 && dy == 0.0 {
        return;
    }

    for elem in elements.iter_mut() {
        match elem {
            ViewElement::Aux(v) => {
                v.x += dx;
                v.y += dy;
            }
            ViewElement::Stock(v) => {
                v.x += dx;
                v.y += dy;
            }
            ViewElement::Flow(v) => {
                v.x += dx;
                v.y += dy;
                for pt in &mut v.points {
                    pt.x += dx;
                    pt.y += dy;
                }
            }
            ViewElement::Link(v) => {
                if let LinkShape::MultiPoint(ref mut pts) = v.shape {
                    for pt in pts {
                        pt.x += dx;
                        pt.y += dy;
                    }
                }
            }
            ViewElement::Module(v) => {
                v.x += dx;
                v.y += dy;
            }
            ViewElement::Alias(v) => {
                v.x += dx;
                v.y += dy;
            }
            ViewElement::Cloud(v) => {
                v.x += dx;
                v.y += dy;
            }
            ViewElement::Group(v) => {
                v.x += dx;
                v.y += dy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element;

    #[test]
    fn test_mid_gap_angle_empty() {
        assert_eq!(mid_gap_angle(&[]), 0.0);
    }

    #[test]
    fn test_mid_gap_angle_single() {
        // Single angle at 0 => opposite is pi
        let result = mid_gap_angle(&[0.0]);
        assert!((result - PI).abs() < 1e-10);

        // Single angle at pi/2 => opposite is 3*pi/2
        let result = mid_gap_angle(&[PI / 2.0]);
        assert!((result - 3.0 * PI / 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_mid_gap_angle_two_angles() {
        // Two angles at 0 and pi/2: gaps are pi/2 and 3*pi/2
        // Largest gap is 3*pi/2 starting at pi/2, midpoint at pi/2 + 3*pi/4 = 5*pi/4
        let result = mid_gap_angle(&[0.0, PI / 2.0]);
        assert!((result - 5.0 * PI / 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_mid_gap_angle_wrap_around() {
        // Angles at 5*pi/4 and 7*pi/4: gap between them = pi/2,
        // wrap-around gap from 7*pi/4 back to 5*pi/4 = 3*pi/2
        // Midpoint of wrap: 7*pi/4 + 3*pi/4 = 10*pi/4 = 5*pi/2 => normalized = pi/2
        let result = mid_gap_angle(&[5.0 * PI / 4.0, 7.0 * PI / 4.0]);
        assert!((result - PI / 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_calculate_optimal_label_side_no_connections() {
        let positions = HashMap::from([("x".to_string(), Position::new(100.0, 100.0))]);
        let uses = BTreeMap::new();
        let used_by = BTreeMap::new();

        assert_eq!(
            calculate_optimal_label_side("x", &positions, &uses, &used_by),
            LabelSide::Bottom
        );
    }

    #[test]
    fn test_calculate_restricted_label_side_prefers_bottom() {
        let positions = HashMap::from([("x".to_string(), Position::new(100.0, 100.0))]);
        let uses = BTreeMap::new();
        let used_by = BTreeMap::new();
        let allowed = [
            LabelSide::Top,
            LabelSide::Bottom,
            LabelSide::Left,
            LabelSide::Right,
        ];

        assert_eq!(
            calculate_restricted_label_side("x", &positions, &uses, &used_by, &allowed),
            LabelSide::Bottom
        );
    }

    #[test]
    fn test_calculate_restricted_label_side_avoids_connections() {
        // Place a connection directly below, so Bottom should be avoided
        let positions = HashMap::from([
            ("x".to_string(), Position::new(100.0, 100.0)),
            ("below".to_string(), Position::new(100.0, 200.0)),
        ]);
        let uses = BTreeMap::new();
        let used_by = BTreeMap::from([("x".to_string(), BTreeSet::from(["below".to_string()]))]);
        let allowed = [
            LabelSide::Top,
            LabelSide::Bottom,
            LabelSide::Left,
            LabelSide::Right,
        ];

        let side = calculate_restricted_label_side("x", &positions, &uses, &used_by, &allowed);
        // Bottom angle (pi/2) is occupied, so it should pick something else
        assert_ne!(side, LabelSide::Bottom);
    }

    #[test]
    fn test_normalize_coordinates_shifts_to_margin() {
        let mut elements = vec![
            ViewElement::Aux(view_element::Aux {
                name: "a".to_string(),
                uid: 1,
                x: 10.0,
                y: 20.0,
                label_side: LabelSide::Bottom,
            }),
            ViewElement::Stock(view_element::Stock {
                name: "s".to_string(),
                uid: 2,
                x: 30.0,
                y: 40.0,
                label_side: LabelSide::Bottom,
            }),
        ];

        normalize_coordinates(&mut elements, 50.0);

        // min was (10, 20), shift is (40, 30)
        match &elements[0] {
            ViewElement::Aux(v) => {
                assert!((v.x - 50.0).abs() < 1e-10);
                assert!((v.y - 50.0).abs() < 1e-10);
            }
            _ => panic!("expected Aux"),
        }
        match &elements[1] {
            ViewElement::Stock(v) => {
                assert!((v.x - 70.0).abs() < 1e-10);
                assert!((v.y - 70.0).abs() < 1e-10);
            }
            _ => panic!("expected Stock"),
        }
    }

    #[test]
    fn test_normalize_coordinates_includes_flow_points() {
        let mut elements = vec![ViewElement::Flow(view_element::Flow {
            name: "f".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: LabelSide::Bottom,
            points: vec![
                view_element::FlowPoint {
                    x: 5.0,
                    y: 10.0,
                    attached_to_uid: None,
                },
                view_element::FlowPoint {
                    x: 200.0,
                    y: 200.0,
                    attached_to_uid: None,
                },
            ],
        })];

        normalize_coordinates(&mut elements, 50.0);

        // min x was 5, min y was 10; shift = (45, 40)
        match &elements[0] {
            ViewElement::Flow(v) => {
                assert!((v.x - 145.0).abs() < 1e-10);
                assert!((v.y - 140.0).abs() < 1e-10);
                assert!((v.points[0].x - 50.0).abs() < 1e-10);
                assert!((v.points[0].y - 50.0).abs() < 1e-10);
                assert!((v.points[1].x - 245.0).abs() < 1e-10);
                assert!((v.points[1].y - 240.0).abs() < 1e-10);
            }
            _ => panic!("expected Flow"),
        }
    }

    #[test]
    fn test_normalize_coordinates_empty() {
        let mut elements: Vec<ViewElement> = vec![];
        normalize_coordinates(&mut elements, 50.0);
        assert!(elements.is_empty());
    }

    #[test]
    fn test_connector_angles_outgoing() {
        let positions = HashMap::from([
            ("a".to_string(), Position::new(0.0, 0.0)),
            ("b".to_string(), Position::new(1.0, 0.0)),
        ]);
        let uses = BTreeMap::new();
        let used_by = BTreeMap::from([("a".to_string(), BTreeSet::from(["b".to_string()]))]);

        let angles = connector_angles("a", &positions, &uses, &used_by);
        assert_eq!(angles.len(), 1);
        assert!(angles[0].abs() < 1e-10); // angle 0 = right
    }

    #[test]
    fn test_connector_angles_incoming_reversed() {
        let positions = HashMap::from([
            ("a".to_string(), Position::new(0.0, 0.0)),
            ("b".to_string(), Position::new(1.0, 0.0)),
        ]);
        // "a" uses "b", so incoming from b: reversed direction is a-b = (-1,0), angle = pi
        let uses = BTreeMap::from([("a".to_string(), BTreeSet::from(["b".to_string()]))]);
        let used_by = BTreeMap::new();

        let angles = connector_angles("a", &positions, &uses, &used_by);
        assert_eq!(angles.len(), 1);
        assert!((angles[0] - PI).abs() < 1e-10);
    }

    #[test]
    fn test_angle_to_label_side_mapping() {
        assert_eq!(angle_to_label_side(0.0), LabelSide::Right);
        assert_eq!(angle_to_label_side(PI / 2.0), LabelSide::Bottom);
        assert_eq!(angle_to_label_side(PI), LabelSide::Left);
        assert_eq!(angle_to_label_side(3.0 * PI / 2.0), LabelSide::Top);
    }
}
