// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of the flow invariant checker.
//!
//! Rows are derived from `FlowArm::ALL`: every arm has a minimal valid fixture
//! strict mode accepts and a minimal mutation it reports. A mutation's expected
//! arm list is exact (a multiset), so a row pins what its arm reports AND that
//! no other arm fires on it; where a mutation cannot violate one arm without
//! another (an inward stub necessarily crosses the body) the row lists both and
//! says why. Each row also states what tolerant mode reports on the same
//! mutation. The boundary table pins each threshold at the value the plan names
//! -- equality on both sides of every `<` / `<=` choice, sub-pixel defects
//! against the epsilon, the precondition inflations at the exact touching
//! distance, one row per face where an arm branches per face -- and the
//! definition tests pin the readings the plan's wording needed.
//!
//! Fixtures are engine JSON loaded through the production conversion. The one
//! exception is the non-finite row, which mutates a loaded view: JSON has no
//! spelling for NaN, so a NaN exists only in memory, where a planner computing
//! it would put it.

use std::collections::HashSet;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::Flow;
use crate::editing::test_support::{aux, cloud, flow, load, stock};
use crate::editing::{self, Face};
use crate::json;

use super::*;

const STRICT: Mode<'static> = Mode::Strict { routed: None };

fn s1() -> json::ViewElement {
    // Faces x = 77.5 / 122.5, y = 82.5 / 117.5.
    stock(1, 100.0, 100.0)
}

/// A straight flow out of S1's right face into a cloud, valve mid-path.
fn straight() -> Vec<json::ViewElement> {
    vec![
        s1(),
        flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
        cloud(3, 2, 300.0, 100.0),
    ]
}

/// An L out of S1's right face turning down into a cloud.
fn l_down() -> Vec<json::ViewElement> {
    vec![
        s1(),
        flow(
            2,
            (200.0, 175.0),
            &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 250.0, 3)],
        ),
        cloud(3, 2, 200.0, 250.0),
    ]
}

fn cloud_straight() -> Vec<json::ViewElement> {
    vec![
        flow(10, (50.0, 0.0), &[(0.0, 0.0, 11), (100.0, 0.0, 12)]),
        cloud(11, 10, 0.0, 0.0),
        cloud(12, 10, 100.0, 0.0),
    ]
}

fn plus(
    mut base: Vec<json::ViewElement>,
    extra: impl IntoIterator<Item = json::ViewElement>,
) -> Vec<json::ViewElement> {
    base.extend(extra);
    base
}

fn map_flow(
    mut view: Vec<ViewElement>,
    uid: i32,
    edit: impl FnOnce(&mut Flow),
) -> Vec<ViewElement> {
    let target = view.iter_mut().find_map(|e| match e {
        ViewElement::Flow(f) if f.uid == uid => Some(f),
        _ => None,
    });
    edit(target.unwrap_or_else(|| panic!("no flow {uid}")));
    view
}

fn names(arms: impl IntoIterator<Item = FlowArm>) -> String {
    let mut names: Vec<&str> = arms.into_iter().map(FlowArm::name).collect();
    names.sort_unstable();
    names.join(", ")
}

fn reported(view: &[ViewElement], mode: Mode<'_>) -> String {
    names(check_flow_invariants(view, mode).iter().map(|v| v.arm))
}

struct ArmRow {
    valid: Vec<json::ViewElement>,
    broken: Vec<ViewElement>,
    /// Every arm strict mode reports on `broken`, one entry per occurrence.
    expected: Vec<FlowArm>,
    /// Every arm tolerant mode reports on `broken`, one entry per occurrence.
    tolerant: Vec<FlowArm>,
    uid: i32,
    numbers: &'static [(&'static str, f64)],
}

/// The fixture row of one arm. The match is exhaustive, so a new arm does not
/// compile until it has a row.
fn arm_row(arm: FlowArm) -> ArmRow {
    use FlowArm::*;
    let row = |valid, broken, expected: &[FlowArm], tolerant: &[FlowArm], uid, numbers| ArmRow {
        valid,
        broken,
        expected: expected.to_vec(),
        tolerant: tolerant.to_vec(),
        uid,
        numbers,
    };
    match arm {
        MinPoints => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (122.5, 100.0), &[(122.5, 100.0, 1)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[MinPoints],
            &[MinPoints],
            2,
            &[("points", 1.0)],
        ),
        NonFinite => row(
            straight(),
            map_flow(load(straight()), 2, |f| f.x = f64::NAN),
            &[NonFinite],
            &[NonFinite],
            2,
            &[("count", 1.0)],
        ),
        UnattachedEndpoint => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 0)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[UnattachedEndpoint],
            &[],
            2,
            &[("endIndex", 1.0)],
        ),
        DanglingAttachment => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 99)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[DanglingAttachment],
            &[DanglingAttachment],
            2,
            &[("endIndex", 1.0), ("attachedToUid", 99.0)],
        ),
        AttachmentKind => row(
            plus(straight(), [aux(4, 400.0, 400.0)]),
            load(vec![
                s1(),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 4)]),
                cloud(3, 2, 300.0, 100.0),
                aux(4, 300.0, 100.0),
            ]),
            &[AttachmentKind],
            &[AttachmentKind],
            2,
            &[("attachedToUid", 4.0)],
        ),
        ForeignCloud => row(
            plus(straight(), cloud_straight()),
            load(plus(
                vec![
                    s1(),
                    flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                    cloud(3, 10, 300.0, 100.0),
                ],
                cloud_straight(),
            )),
            &[ForeignCloud],
            &[ForeignCloud],
            2,
            &[("cloudUid", 3.0), ("cloudFlowUid", 10.0)],
        ),
        InteriorAttached => row(
            l_down(),
            load(vec![
                s1(),
                flow(
                    2,
                    (200.0, 175.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 1), (200.0, 250.0, 3)],
                ),
                cloud(3, 2, 200.0, 250.0),
            ]),
            &[InteriorAttached],
            &[InteriorAttached],
            2,
            &[("index", 1.0), ("attachedToUid", 1.0)],
        ),
        // -3 is a planner's faux-target sentinel, the kind of uid that must
        // never reach a committed view.
        NonPositiveUid => row(
            plus(straight(), [aux(4, 400.0, 400.0)]),
            load(plus(straight(), [aux(-3, 400.0, 400.0)])),
            &[NonPositiveUid],
            &[],
            -3,
            &[("uid", -3.0)],
        ),
        // Out of S1's right face, around, and back into its top face. The
        // terminals overlap themselves, so the G3 minima and G6 are exempt; only
        // G1 reports.
        SourceIsSink => row(
            vec![
                s1(),
                stock(4, 300.0, 100.0),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (277.5, 100.0, 4)]),
            ],
            load(vec![
                s1(),
                flow(
                    2,
                    (160.0, 70.0),
                    &[
                        (122.5, 100.0, 1),
                        (160.0, 100.0, 0),
                        (160.0, 40.0, 0),
                        (100.0, 40.0, 0),
                        (100.0, 82.5, 1),
                    ],
                ),
            ]),
            &[SourceIsSink],
            &[],
            2,
            &[("attachedToUid", 1.0)],
        ),
        Diagonal => row(
            cloud_straight(),
            load(vec![
                flow(10, (50.0, 1.5), &[(0.0, 0.0, 11), (100.0, 3.0, 12)]),
                cloud(11, 10, 0.0, 0.0),
                cloud(12, 10, 100.0, 3.0),
            ]),
            &[Diagonal],
            &[],
            10,
            &[("segment", 0.0), ("dx", 100.0), ("dy", 3.0)],
        ),
        ZeroLength => row(
            l_down(),
            load(vec![
                s1(),
                flow(
                    2,
                    (200.0, 175.0),
                    &[
                        (122.5, 100.0, 1),
                        (200.0, 100.0, 0),
                        (200.0, 100.0, 0),
                        (200.0, 250.0, 3),
                    ],
                ),
                cloud(3, 2, 200.0, 250.0),
            ]),
            &[ZeroLength],
            &[],
            2,
            &[("segment", 1.0)],
        ),
        Collinear => row(
            straight(),
            load(vec![
                s1(),
                flow(
                    2,
                    (200.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (300.0, 100.0, 3)],
                ),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[Collinear],
            &[],
            2,
            &[("segment", 1.0)],
        ),
        ShortStub => row(
            vec![
                flow(
                    10,
                    (50.0, 175.0),
                    &[(0.0, 100.0, 11), (50.0, 100.0, 0), (50.0, 250.0, 12)],
                ),
                cloud(11, 10, 0.0, 100.0),
                cloud(12, 10, 50.0, 250.0),
            ],
            load(vec![
                flow(
                    10,
                    (5.0, 175.0),
                    &[(0.0, 100.0, 11), (5.0, 100.0, 0), (5.0, 250.0, 12)],
                ),
                cloud(11, 10, 0.0, 100.0),
                cloud(12, 10, 5.0, 250.0),
            ]),
            &[ShortStub],
            &[],
            10,
            &[("segment", 0.0), ("length", 5.0), ("minimum", 10.0)],
        ),
        ShortRiser => row(
            vec![
                flow(
                    10,
                    (50.0, 100.0),
                    &[
                        (0.0, 100.0, 11),
                        (100.0, 100.0, 0),
                        (100.0, 150.0, 0),
                        (200.0, 150.0, 12),
                    ],
                ),
                cloud(11, 10, 0.0, 100.0),
                cloud(12, 10, 200.0, 150.0),
            ],
            load(vec![
                flow(
                    10,
                    (50.0, 100.0),
                    &[
                        (0.0, 100.0, 11),
                        (100.0, 100.0, 0),
                        (100.0, 104.0, 0),
                        (200.0, 104.0, 12),
                    ],
                ),
                cloud(11, 10, 0.0, 100.0),
                cloud(12, 10, 200.0, 104.0),
            ]),
            &[ShortRiser],
            &[],
            10,
            &[("segment", 1.0), ("length", 4.0), ("minimum", 10.0)],
        ),
        ShortSink => row(
            vec![
                s1(),
                flow(
                    2,
                    (160.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 250.0, 3)],
                ),
                cloud(3, 2, 200.0, 250.0),
            ],
            load(vec![
                s1(),
                flow(
                    2,
                    (160.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 110.0, 3)],
                ),
                cloud(3, 2, 200.0, 110.0),
            ]),
            &[ShortSink],
            &[],
            2,
            &[("segment", 1.0), ("length", 10.0), ("minimum", 15.5)],
        ),
        OffFace => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 100.0), &[(130.0, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[OffFace],
            &[],
            2,
            &[("endIndex", 0.0), ("dx", 30.0), ("dy", 0.0)],
        ),
        CornerClearance => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 83.5), &[(122.5, 83.5, 1), (300.0, 83.5, 3)]),
                cloud(3, 2, 300.0, 83.5),
            ]),
            &[CornerClearance],
            &[],
            2,
            &[("endIndex", 0.0), ("clearance", 1.0), ("minimum", 3.0)],
        ),
        NotPerpendicular => row(
            vec![
                s1(),
                flow(
                    2,
                    (160.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 40.0, 3)],
                ),
                cloud(3, 2, 200.0, 40.0),
            ],
            load(vec![
                s1(),
                flow(
                    2,
                    (200.0, 40.0),
                    &[(122.5, 100.0, 1), (122.5, 40.0, 0), (300.0, 40.0, 3)],
                ),
                cloud(3, 2, 300.0, 40.0),
            ]),
            &[NotPerpendicular],
            &[],
            2,
            &[("endIndex", 0.0), ("dx", 0.0), ("dy", -60.0)],
        ),
        // A stub pointing into the stock necessarily crosses its body
        // (segments 0 and 1), so G6 fires too.
        Inward => row(
            straight(),
            load(vec![
                s1(),
                flow(
                    2,
                    (110.0, 200.0),
                    &[(122.5, 100.0, 1), (110.0, 100.0, 0), (110.0, 300.0, 3)],
                ),
                cloud(3, 2, 110.0, 300.0),
            ]),
            &[Inward, SegmentThroughTerminal, SegmentThroughTerminal],
            &[],
            2,
            &[("endIndex", 0.0), ("dx", -12.5), ("dy", 0.0)],
        ),
        SegmentThroughTerminal => row(
            vec![
                s1(),
                flow(
                    2,
                    (60.0, 200.0),
                    &[
                        (122.5, 110.0, 1),
                        (140.0, 110.0, 0),
                        (140.0, 60.0, 0),
                        (60.0, 60.0, 0),
                        (60.0, 300.0, 3),
                    ],
                ),
                cloud(3, 2, 60.0, 300.0),
            ],
            load(vec![
                s1(),
                flow(
                    2,
                    (60.0, 200.0),
                    &[
                        (122.5, 110.0, 1),
                        (140.0, 110.0, 0),
                        (140.0, 95.0, 0),
                        (60.0, 95.0, 0),
                        (60.0, 300.0, 3),
                    ],
                ),
                cloud(3, 2, 60.0, 300.0),
            ]),
            &[SegmentThroughTerminal],
            &[],
            2,
            &[("segment", 2.0), ("stockUid", 1.0)],
        ),
        CloudInsideStock => row(
            vec![
                s1(),
                flow(
                    10,
                    (300.0, 100.0),
                    &[(200.0, 100.0, 11), (400.0, 100.0, 12)],
                ),
                cloud(11, 10, 200.0, 100.0),
                cloud(12, 10, 400.0, 100.0),
            ],
            load(vec![
                s1(),
                flow(
                    10,
                    (200.0, 100.0),
                    &[(100.0, 100.0, 11), (300.0, 100.0, 12)],
                ),
                cloud(11, 10, 100.0, 100.0),
                cloud(12, 10, 300.0, 100.0),
            ]),
            &[CloudInsideStock],
            &[],
            10,
            &[("cloudUid", 11.0), ("stockUid", 1.0)],
        ),
        CloudOffEndpoint => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 305.0, 100.0),
            ]),
            &[CloudOffEndpoint],
            &[],
            2,
            &[("endIndex", 1.0), ("distance", 5.0)],
        ),
        ValveOffPath => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (200.0, 105.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[ValveOffPath],
            &[],
            2,
            &[("distance", 5.0)],
        ),
        ValveMargin => row(
            straight(),
            load(vec![
                s1(),
                flow(2, (295.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 300.0, 100.0),
            ]),
            &[ValveMargin],
            &[],
            2,
            &[
                ("arcPosition", 172.5),
                ("pathLength", 177.5),
                ("margin", 10.0),
            ],
        ),
    }
}

#[test]
fn all_lists_every_arm_once_in_declaration_order() {
    // Exhaustive: a new arm does not compile until it is placed, and placing it
    // past the end of ALL fails the check below.
    fn position(arm: FlowArm) -> usize {
        use FlowArm::*;
        match arm {
            MinPoints => 0,
            NonFinite => 1,
            UnattachedEndpoint => 2,
            DanglingAttachment => 3,
            AttachmentKind => 4,
            ForeignCloud => 5,
            InteriorAttached => 6,
            NonPositiveUid => 7,
            SourceIsSink => 8,
            Diagonal => 9,
            ZeroLength => 10,
            Collinear => 11,
            ShortStub => 12,
            ShortRiser => 13,
            ShortSink => 14,
            OffFace => 15,
            CornerClearance => 16,
            NotPerpendicular => 17,
            Inward => 18,
            SegmentThroughTerminal => 19,
            CloudInsideStock => 20,
            CloudOffEndpoint => 21,
            ValveOffPath => 22,
            ValveMargin => 23,
        }
    }
    for (i, arm) in FlowArm::ALL.iter().enumerate() {
        assert_eq!(position(*arm), i, "{} is out of place in ALL", arm.name());
    }
    let distinct: HashSet<&str> = FlowArm::ALL.iter().map(|a| a.name()).collect();
    assert_eq!(distinct.len(), FlowArm::ALL.len());
}

#[test]
fn every_arm_has_a_valid_fixture_and_a_mutation_that_reports_exactly_its_arms() {
    let mut failures = Vec::new();
    for arm in FlowArm::ALL {
        let row = arm_row(arm);
        let name = arm.name();
        let valid = check_flow_invariants(&load(row.valid), STRICT);
        if !valid.is_empty() {
            failures.push(format!(
                "{name}: the valid fixture reports\n{}",
                format_violations(&valid)
            ));
        }
        let broken = check_flow_invariants(&row.broken, STRICT);
        let got = names(broken.iter().map(|v| v.arm));
        let want = names(row.expected.iter().copied());
        if got != want {
            failures.push(format!(
                "{name}: strict mode reports [{got}], want [{want}]"
            ));
        }
        match broken.iter().find(|v| v.arm == arm) {
            None => failures.push(format!("{name}: no violation of the row's own arm")),
            Some(target) => {
                if target.uid != row.uid {
                    failures.push(format!(
                        "{name}: reported on uid {}, want {}",
                        target.uid, row.uid
                    ));
                }
                for &(key, value) in row.numbers {
                    match target.number(key) {
                        Some(n) if (n - value).abs() <= 1e-9 => {}
                        other => failures
                            .push(format!("{name}: number {key} is {other:?}, want {value}")),
                    }
                }
            }
        }
        let tolerant = reported(&row.broken, Mode::Tolerant);
        let want_tolerant = names(row.tolerant.iter().copied());
        if tolerant != want_tolerant {
            failures.push(format!(
                "{name}: tolerant mode reports [{tolerant}], want [{want_tolerant}]"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// G6's precondition inflates both terminals by `MIN_SEGMENT`. S1's inflated
/// body starts at x = 67.5 and a sink cloud at `cx` inflated by `MIN_SEGMENT`
/// ends at cx + 10, so cx = 57.5 touches (the precondition holds) and 57.6
/// overlaps. The cloud crowds S1 for G3's room either way, so only G6 reports.
fn g6_precondition(cx: f64) -> Vec<json::ViewElement> {
    vec![
        s1(),
        flow(
            2,
            (100.0, 95.0),
            &[
                (122.5, 110.0, 1),
                (140.0, 110.0, 0),
                (140.0, 95.0, 0),
                (cx, 95.0, 0),
                (cx, 100.0, 3),
            ],
        ),
        cloud(3, 2, cx, 100.0),
    ]
}

/// G3's room inflates the source by `MIN_SEGMENT` (S1 to x = 132.5) and the sink
/// by `MIN_SINK_SEGMENT` (a cloud at `cx` from cx - 15.5), so cx = 148 touches
/// (room: the 10px final segment reports) and 147.9 overlaps (exempt).
fn g3_room(cx: f64) -> Vec<json::ViewElement> {
    vec![
        s1(),
        flow(
            2,
            (135.0, 100.0),
            &[(122.5, 100.0, 1), (cx, 100.0, 0), (cx, 110.0, 3)],
        ),
        cloud(3, 2, cx, 110.0),
    ]
}

/// A diagonal first segment that still moves away from the face: the outward
/// test must demand an axis-aligned segment, not only the outward sign.
fn diagonal_exit(face: Face) -> ((f64, f64), (f64, f64)) {
    match face {
        Face::Left => ((77.5, 100.0), (37.5, 80.0)),
        Face::Right => ((122.5, 100.0), (162.5, 80.0)),
        Face::Top => ((100.0, 82.5), (80.0, 42.5)),
        Face::Bottom => ((100.0, 117.5), (80.0, 157.5)),
    }
}

#[test]
fn thresholds_sit_where_the_plan_names_them() {
    use FlowArm::*;
    let mut rows: Vec<(String, Vec<json::ViewElement>, Vec<FlowArm>)> = vec![
        (
            "G1: uid 0 is non-positive".into(),
            plus(straight(), [aux(0, 400.0, 400.0)]),
            vec![NonPositiveUid],
        ),
        (
            "G2: a 1e-3px drift across the axis is diagonal".into(),
            vec![
                flow(10, (50.0, 0.0005), &[(0.0, 0.0, 11), (100.0, 0.001, 12)]),
                cloud(11, 10, 0.0, 0.0),
                cloud(12, 10, 100.0, 0.001),
            ],
            vec![Diagonal],
        ),
        (
            "G3: two consecutive vertical segments are collinear".into(),
            vec![
                flow(
                    10,
                    (0.0, 25.0),
                    &[(0.0, 0.0, 11), (0.0, 50.0, 0), (0.0, 100.0, 12)],
                ),
                cloud(11, 10, 0.0, 0.0),
                cloud(12, 10, 0.0, 100.0),
            ],
            vec![Collinear],
        ),
        (
            "G3: room when the inflated terminals exactly touch".into(),
            g3_room(148.0),
            vec![ShortSink],
        ),
        (
            "G3: no room when they overlap by 0.1px".into(),
            g3_room(147.9),
            vec![],
        ),
        (
            "G3: a missing terminal cannot crowd, so the minima still apply".into(),
            vec![
                s1(),
                flow(
                    2,
                    (160.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 110.0, 0)],
                ),
            ],
            vec![UnattachedEndpoint, ShortSink],
        ),
        (
            "G3: a final segment of exactly MIN_SINK_SEGMENT is long enough".into(),
            vec![
                s1(),
                flow(
                    2,
                    (160.0, 100.0),
                    &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 115.5, 3)],
                ),
                cloud(3, 2, 200.0, 115.5),
            ],
            vec![],
        ),
        (
            "G3: a riser of exactly MIN_SEGMENT is long enough".into(),
            vec![
                flow(
                    10,
                    (50.0, 100.0),
                    &[
                        (0.0, 100.0, 11),
                        (100.0, 100.0, 0),
                        (100.0, 110.0, 0),
                        (200.0, 110.0, 12),
                    ],
                ),
                cloud(11, 10, 0.0, 100.0),
                cloud(12, 10, 200.0, 110.0),
            ],
            vec![],
        ),
        (
            "G4: an endpoint on the face line 2.5px past the corner is off the face".into(),
            vec![
                s1(),
                flow(2, (200.0, 120.0), &[(122.5, 120.0, 1), (300.0, 120.0, 3)]),
                cloud(3, 2, 300.0, 120.0),
            ],
            vec![OffFace],
        ),
        (
            "G6: a segment through the SINK terminal reports".into(),
            vec![
                s1(),
                flow(
                    2,
                    (60.0, 200.0),
                    &[
                        (60.0, 300.0, 3),
                        (60.0, 95.0, 0),
                        (140.0, 95.0, 0),
                        (140.0, 110.0, 0),
                        (122.5, 110.0, 1),
                    ],
                ),
                cloud(3, 2, 60.0, 300.0),
            ],
            vec![SegmentThroughTerminal],
        ),
        (
            "G6: the precondition holds when the bodies inflated by MIN_SEGMENT exactly touch"
                .into(),
            g6_precondition(57.5),
            vec![SegmentThroughTerminal],
        ),
        (
            "G6: overlapping by 0.1px exempts the crossing".into(),
            g6_precondition(57.6),
            vec![],
        ),
        (
            "G6: a segment 1.5px inside the body crosses it".into(),
            vec![
                s1(),
                flow(
                    2,
                    (60.0, 200.0),
                    &[
                        (122.5, 110.0, 1),
                        (140.0, 110.0, 0),
                        (140.0, 84.0, 0),
                        (60.0, 84.0, 0),
                        (60.0, 300.0, 3),
                    ],
                ),
                cloud(3, 2, 60.0, 300.0),
            ],
            vec![SegmentThroughTerminal],
        ),
        (
            "G6: a cloud exactly on a stock edge is not inside it".into(),
            vec![
                s1(),
                flow(
                    10,
                    (200.0, 100.0),
                    &[(122.5, 100.0, 11), (300.0, 100.0, 12)],
                ),
                cloud(11, 10, 122.5, 100.0),
                cloud(12, 10, 300.0, 100.0),
            ],
            vec![],
        ),
        (
            "G7: a cloud 1e-3px off its endpoint is off".into(),
            vec![
                s1(),
                flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 300.001, 100.0),
            ],
            vec![CloudOffEndpoint],
        ),
        (
            "G8: a valve 1e-3px off the path is off".into(),
            vec![
                s1(),
                flow(2, (200.0, 100.001), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
                cloud(3, 2, 300.0, 100.0),
            ],
            vec![ValveOffPath],
        ),
        (
            "G8: a valve exactly VALVE_CLAMP_MARGIN from an end is far enough".into(),
            vec![
                flow(10, (10.0, 0.0), &[(0.0, 0.0, 11), (100.0, 0.0, 12)]),
                cloud(11, 10, 0.0, 0.0),
                cloud(12, 10, 100.0, 0.0),
            ],
            vec![],
        ),
    ];
    for face in Face::ALL {
        let (start, end) = diagonal_exit(face);
        rows.push((
            format!(
                "G5: a diagonal segment moving away from the {face:?} face is not an outward exit"
            ),
            vec![
                s1(),
                flow(
                    2,
                    ((start.0 + end.0) / 2.0, (start.1 + end.1) / 2.0),
                    &[(start.0, start.1, 1), (end.0, end.1, 3)],
                ),
                cloud(3, 2, end.0, end.1),
            ],
            vec![Diagonal, NotPerpendicular],
        ));
    }
    let failures: Vec<String> = rows
        .into_iter()
        .filter_map(|(name, elements, expected)| {
            let got = reported(&load(elements), STRICT);
            let want = names(expected);
            (got != want).then(|| format!("{name}: reports [{got}], want [{want}]"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn crowded_terminals_exempt_the_segment_minima() {
    // A 12.5px straight flow between stocks.
    let view = load(vec![
        stock(1, 100.0, 100.0),
        stock(2, 157.5, 100.0),
        flow(4, (128.75, 100.0), &[(122.5, 100.0, 1), (135.0, 100.0, 2)]),
    ]);
    assert_eq!(reported(&view, STRICT), "");
}

#[test]
fn overlapping_inflated_terminal_bodies_exempt_a_body_crossing() {
    // The G6 row's crossing route with the sink cloud within MIN_SEGMENT of the stock.
    let view = load(vec![
        s1(),
        flow(
            2,
            (60.0, 110.0),
            &[
                (122.5, 110.0, 1),
                (140.0, 110.0, 0),
                (140.0, 95.0, 0),
                (60.0, 95.0, 0),
                (60.0, 125.0, 3),
            ],
        ),
        cloud(3, 2, 60.0, 125.0),
    ]);
    assert_eq!(reported(&view, STRICT), "");
}

#[test]
fn the_exit_is_undefined_for_an_off_face_endpoint() {
    let view = load(vec![
        s1(),
        flow(2, (130.0, 180.0), &[(130.0, 60.0, 1), (130.0, 300.0, 3)]),
        cloud(3, 2, 130.0, 300.0),
    ]);
    assert_eq!(reported(&view, STRICT), "G4.offFace");
}

#[test]
fn the_exit_reads_the_first_segment_of_positive_length_past_a_coincident_point() {
    let view = load(vec![
        s1(),
        flow(
            2,
            (200.0, 40.0),
            &[
                (122.5, 100.0, 1),
                (122.5, 100.0, 0),
                (122.5, 40.0, 0),
                (300.0, 40.0, 3),
            ],
        ),
        cloud(3, 2, 300.0, 40.0),
    ]);
    assert_eq!(
        reported(&view, STRICT),
        "G3.zeroLength, G5.notPerpendicular"
    );
}

#[test]
fn an_endpoint_exactly_on_a_corner_reports_the_clearance_but_not_the_exit() {
    let view = load(vec![
        s1(),
        flow(2, (200.0, 82.5), &[(122.5, 82.5, 1), (300.0, 82.5, 3)]),
        cloud(3, 2, 300.0, 82.5),
    ]);
    let violations = check_flow_invariants(&view, STRICT);
    assert_eq!(
        names(violations.iter().map(|v| v.arm)),
        "G4.cornerClearance"
    );
    assert_eq!(violations[0].number("clearance"), Some(0.0));
}

#[test]
fn the_valve_margin_applies_from_two_margins_of_arc_length() {
    let cloud_flow = |valve_x: f64, length: f64| {
        load(vec![
            flow(10, (valve_x, 0.0), &[(0.0, 0.0, 11), (length, 0.0, 12)]),
            cloud(11, 10, 0.0, 0.0),
            cloud(12, 10, length, 0.0),
        ])
    };
    // Shorter than two margins: no requirement.
    assert_eq!(reported(&cloud_flow(2.0, 15.0), STRICT), "");
    // Exactly two margins (the G3 minima are exempt: the clouds crowd each
    // other): the requirement already applies to a valve 5px from the source.
    assert_eq!(reported(&cloud_flow(5.0, 20.0), STRICT), "G8.valveMargin");
    // Arc length from the path's ends, not from segment ends: a valve 4px past
    // an L's corner is 81.5px along the path.
    let l_valve = load(vec![
        s1(),
        flow(
            2,
            (200.0, 104.0),
            &[(122.5, 100.0, 1), (200.0, 100.0, 0), (200.0, 250.0, 3)],
        ),
        cloud(3, 2, 200.0, 250.0),
    ]);
    assert_eq!(reported(&l_valve, STRICT), "");
}

#[test]
fn strict_arms_apply_only_to_routed_flows_and_the_rest_keep_the_tolerant_arms() {
    let view = load(vec![
        // Flow 2 (routed): its cloud 5px off its endpoint.
        s1(),
        flow(2, (200.0, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 3)]),
        cloud(3, 2, 305.0, 100.0),
        // Flow 10 (not routed): the same geometric defect plus a dangling sink.
        flow(10, (50.0, 300.0), &[(0.0, 300.0, 11), (100.0, 300.0, 99)]),
        cloud(11, 10, 3.0, 300.0),
    ]);
    let routed: HashSet<i32> = [2].into_iter().collect();
    let mut got: Vec<String> = check_flow_invariants(
        &view,
        Mode::Strict {
            routed: Some(&routed),
        },
    )
    .iter()
    .map(|v| format!("{}:{}", v.uid, v.arm.name()))
    .collect();
    got.sort();
    assert_eq!(got, ["10:G1.danglingAttachment", "2:G7.cloudOffEndpoint"]);
}

#[test]
fn an_empty_routed_set_still_demands_positive_uids() {
    let view = load(plus(straight(), [aux(-3, 400.0, 400.0)]));
    let routed = HashSet::new();
    assert_eq!(
        reported(
            &view,
            Mode::Strict {
                routed: Some(&routed)
            }
        ),
        "G1.nonPositiveUid"
    );
    assert_eq!(reported(&view, Mode::Tolerant), "");
}

#[test]
fn tolerant_mode_accepts_an_unattached_flow() {
    // Vensim fallback flows import this way.
    let view = load(vec![flow(
        10,
        (50.0, 0.0),
        &[(0.0, 0.0, 0), (100.0, 0.0, 0)],
    )]);
    assert_eq!(reported(&view, Mode::Tolerant), "");
    assert_eq!(
        reported(&view, STRICT),
        "G1.unattachedEndpoint, G1.unattachedEndpoint"
    );
}

#[test]
fn the_tolerant_arms_are_the_structural_arms_heal_cannot_repair() {
    assert_eq!(
        names(FlowArm::ALL.into_iter().filter(|a| a.tolerant())),
        "G1.attachmentKind, G1.danglingAttachment, G1.foreignCloud, G1.interiorAttached, G1.minPoints, G1.nonFinite"
    );
}

#[test]
fn the_checker_constants_are_the_plan_literals_and_the_cores() {
    // The checker keeps its own literals so a changed core constant cannot move
    // the oracle along with it; this pins all three against each other. Pipe
    // spacing is a slot preference the core falls back from, not an invariant,
    // so the checker has no literal for it.
    let plan = [3.0, 10.0, 10.0, 15.5, 1e-6];
    let checker = [
        CORNER_CLEARANCE,
        MIN_SEGMENT,
        VALVE_CLAMP_MARGIN,
        MIN_SINK_SEGMENT,
        GEOMETRY_EPSILON,
    ];
    let core = [
        editing::CORNER_CLEARANCE,
        editing::MIN_SEGMENT,
        editing::VALVE_CLAMP_MARGIN,
        editing::MIN_SINK_SEGMENT,
        editing::GEOMETRY_EPSILON,
    ];
    assert_eq!(checker, plan);
    assert_eq!(core, plan);
}
