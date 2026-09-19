// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of the hit index's prune.
//!
//! The index adds one decision to hit testing: which elements a point is tested
//! against. The tiers walk the elements from the topmost down, and an element
//! that offers a point nothing changes nothing in that walk, so the prune is
//! right exactly when the hit decided over the elements within reach -- with
//! the rule that decided it -- is the hit decided over every element. These
//! tests compare the two over generated rows:
//!
//! - scenes: `scene_gen`'s strict and imported scenes, crowded with auxes and
//!   aliases dropped among what they draw, framed by groups, with every label
//!   side and some variables arrayed; a scene of the two tier arms no generated
//!   scene reliably holds (an end handle drawn above a valve, a label drawn
//!   above a stock); a scene of elements the grid cannot place (a non-finite
//!   coordinate, a coordinate beyond any view, a group too large to list cell by
//!   cell); and World3 from the corpus;
//! - points: pseudo-random over each scene, on rings around every element's
//!   position and every flow point, and just inside and outside the boxes the
//!   index placed;
//! - tolerances: zero, a pointer's and a finger's slop, zoomed-out reaches that
//!   the grid still answers and one it leaves to the walk over every element,
//!   and negative and non-finite values.
//!
//! Coverage is asserted over what was decided, derived from the enumerations:
//! every `HitPart`, every `Rule`, every element kind, answers from the grid and
//! from the walk over every element, and an element listed everywhere. What
//! this does not establish: that the tiers rank rightly, which `hit_tests` pins
//! row by row, or that a host's slop is right.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::datamodel::view_element::{self, LabelSide};
use crate::datamodel::{self, Equation, Variable, ViewElement};
use crate::editing::base::position_of;
use crate::editing::scene_gen::{Rng, imported_scene, strict_scene};
use crate::editing::test_support::{aux, cloud, flow, labeled, load, project_of, stock, view_of};
use crate::test_common::TestProject;

use super::*;

const SEEDS: u32 = 40;

/// Zero; a pointer's and a finger's slop; a zoomed-out reach the grid answers
/// from many cells; one it leaves to the walk over every element; and a
/// negative and a non-finite value, which the tiers read as zero.
const TOLERANCES: [f64; 7] = [0.0, 6.0, 14.0, 300.0, 2000.0, -3.0, f64::NAN];

/// Every kind of element, by the name `kind` gives it.
const KINDS: [&str; 8] = [
    "group", "link", "flow", "stock", "cloud", "module", "aux", "alias",
];

const SIDES: [LabelSide; 5] = [
    LabelSide::Top,
    LabelSide::Left,
    LabelSide::Center,
    LabelSide::Bottom,
    LabelSide::Right,
];

/// How much of a scene a sweep samples.
struct Sampling {
    random: usize,
    radii: &'static [f64],
    angles: usize,
    max_boxes: usize,
    offsets: &'static [f64],
    tolerances: &'static [f64],
}

const SCENE: Sampling = Sampling {
    random: 100,
    radii: &[0.0, 4.0, 9.0, 20.0, 45.0],
    angles: 1,
    max_boxes: 32,
    offsets: &[-1.0, 0.0, 8.5, 14.5],
    tolerances: &TOLERANCES,
};

/// World3 is large, and a walk over every element in a debug build is slow, so
/// it is sampled within a debug build's per-test budget (`docs/dev/rust.md`):
/// one ring around each position, and a finger's slop and a zoomed-out reach,
/// with box offsets on the edge, just past a finger's end handle and just past
/// its tolerance. The generated scenes sweep every tolerance.
const CORPUS: Sampling = Sampling {
    random: 300,
    radii: &[9.0],
    angles: 1,
    max_boxes: 128,
    offsets: &[0.0, 8.5, 14.5],
    tolerances: &[14.0, 300.0],
};

fn kind(element: &ViewElement) -> &'static str {
    match element {
        ViewElement::Group(_) => "group",
        ViewElement::Link(_) => "link",
        ViewElement::Flow(_) => "flow",
        ViewElement::Stock(_) => "stock",
        ViewElement::Cloud(_) => "cloud",
        ViewElement::Module(_) => "module",
        ViewElement::Aux(_) => "aux",
        ViewElement::Alias(_) => "alias",
    }
}

#[derive(Default)]
struct Tally {
    decisions: usize,
    differing: usize,
    examples: Vec<String>,
    parts: HashSet<HitPart>,
    rules: HashSet<Rule>,
    kinds: HashSet<&'static str>,
    listed: bool,
    all: bool,
    everywhere: bool,
}

impl Tally {
    fn merge(mut self, other: Tally) -> Tally {
        self.decisions += other.decisions;
        self.differing += other.differing;
        self.examples.extend(
            other
                .examples
                .into_iter()
                .take(8 - self.examples.len().min(8)),
        );
        self.parts.extend(other.parts);
        self.rules.extend(other.rules);
        self.kinds.extend(other.kinds);
        self.listed |= other.listed;
        self.all |= other.all;
        self.everywhere |= other.everywhere;
        self
    }
}

/// Every element's position and every flow point, read from the view.
fn positions(elements: &[ViewElement]) -> Vec<Point> {
    let mut out = Vec::new();
    for element in elements {
        if let ViewElement::Flow(f) = element {
            out.extend(f.points.iter().map(|p| Point::new(p.x, p.y)));
        }
        out.extend(position_of(element));
    }
    out
}

/// The points a sweep asks about: pseudo-random over the view's positions, on
/// rings around each position, and just inside and outside the boxes the index
/// placed, plus `extra`.
fn sample(
    elements: &[ViewElement],
    index: &HitIndex,
    sampling: &Sampling,
    extra: &[Point],
    rng: &mut Rng,
) -> Vec<Point> {
    let anchors = positions(elements);
    let finite: Vec<&Point> = anchors.iter().filter(|p| p.is_finite()).collect();
    let (mut left, mut top, mut right, mut bottom) = (0.0_f64, 0.0_f64, 1.0_f64, 1.0_f64);
    for p in &finite {
        left = left.min(p.x);
        top = top.min(p.y);
        right = right.max(p.x);
        bottom = bottom.max(p.y);
    }
    let mut points = extra.to_vec();
    for _ in 0..sampling.random {
        points.push(Point::new(
            rng.float(left - 80.0, right + 80.0),
            rng.float(top - 80.0, bottom + 80.0),
        ));
    }
    for p in &anchors {
        for &radius in sampling.radii {
            for _ in 0..sampling.angles {
                let angle = rng.float(0.0, 2.0 * std::f64::consts::PI);
                points.push(Point::new(
                    p.x + radius * angle.cos(),
                    p.y + radius * angle.sin(),
                ));
            }
        }
    }
    let mut boxes = Vec::new();
    for entry in &index.entries {
        entry.shape.for_each_reach_box(|b| boxes.push(b));
    }
    let stride = boxes.len() / sampling.max_boxes + 1;
    for b in boxes.iter().step_by(stride) {
        let (mid_x, mid_y) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
        for &offset in sampling.offsets {
            points.extend([
                Point::new(b.left - offset, mid_y),
                Point::new(b.right + offset, mid_y),
                Point::new(mid_x, b.top - offset),
                Point::new(mid_x, b.bottom + offset),
                Point::new(b.left - offset, b.top - offset),
                Point::new(b.right + offset, b.bottom + offset),
            ]);
        }
    }
    points
}

/// Decide every sampled point at every tolerance both ways, and tally what was
/// decided.
fn sweep(
    project: &datamodel::Project,
    sampling: &Sampling,
    extra: &[Point],
    rng: &mut Rng,
    context: &str,
) -> Tally {
    let index = HitIndex::new(project, "main").expect("main has a view");
    let elements = view_of(project);
    let kinds: HashMap<i32, &'static str> =
        elements.iter().map(|e| (e.get_uid(), kind(e))).collect();
    let points = sample(elements, &index, sampling, extra, rng);
    let mut tally = Tally {
        everywhere: !index.grid.everywhere.is_empty(),
        ..Tally::default()
    };
    for &point in &points {
        for &tolerance in sampling.tolerances {
            tally.decisions += 1;
            if let Some(reach) = Reach::of(point, tolerance) {
                match index.grid.candidates(reach.point, reach.radius()) {
                    Candidates::Listed(_) => tally.listed = true,
                    Candidates::All => tally.all = true,
                }
            }
            let pruned = index.decide(point, tolerance);
            let every = index.decide_over_every_element(point, tolerance);
            if pruned != every {
                tally.differing += 1;
                if tally.examples.len() < 8 {
                    tally.examples.push(format!(
                        "{context}: ({}, {}) at tolerance {tolerance}: pruned {pruned:?}, every element {every:?}",
                        point.x, point.y
                    ));
                }
            }
            if let Some((hit, rule)) = every {
                tally.parts.insert(hit.part);
                tally.rules.insert(rule);
                tally.kinds.extend(kinds.get(&hit.uid).copied());
            }
        }
    }
    tally
}

/// `elements` crowded with auxes and aliases dropped among what they draw,
/// framed by groups, every label on a pseudo-random side, and some variables
/// arrayed: the overlaps real diagrams have and strict scenes keep apart.
fn crowded(mut elements: Vec<ViewElement>, rng: &mut Rng) -> datamodel::Project {
    let mut next_uid = elements.iter().map(ViewElement::get_uid).max().unwrap_or(0) + 1;
    let anchors: Vec<Point> = elements.iter().filter_map(position_of).collect();
    let mut uid = || {
        next_uid += 1;
        next_uid - 1
    };
    let near = |p: Point, rng: &mut Rng| {
        Point::new(p.x + rng.float(-25.0, 25.0), p.y + rng.float(-25.0, 25.0))
    };
    for &p in &anchors {
        if !rng.chance(0.4) {
            continue;
        }
        let crowd = uid();
        let at = near(p, rng);
        elements.push(ViewElement::Aux(view_element::Aux {
            name: format!("Crowd {crowd}"),
            uid: crowd,
            x: at.x,
            y: at.y,
            label_side: rng.pick(&SIDES),
            compat: None,
        }));
        if rng.chance(0.3) {
            let at = near(p, rng);
            elements.push(ViewElement::Alias(view_element::Alias {
                uid: uid(),
                alias_of_uid: crowd,
                x: at.x,
                y: at.y,
                label_side: rng.pick(&SIDES),
                compat: None,
            }));
        }
    }
    for _ in 0..rng.int(1.0, 3.0) as usize {
        let center = rng.pick(&anchors);
        let group = uid();
        elements.push(ViewElement::Group(view_element::Group {
            uid: group,
            name: format!("Sector {group}"),
            x: center.x,
            y: center.y,
            width: rng.float(40.0, 700.0),
            height: rng.float(40.0, 700.0),
            is_mdl_view_marker: false,
        }));
    }
    for element in &mut elements {
        let side = rng.pick(&SIDES);
        match element {
            ViewElement::Stock(e) => e.label_side = side,
            ViewElement::Flow(e) => e.label_side = side,
            ViewElement::Aux(e) => e.label_side = side,
            ViewElement::Module(e) => e.label_side = side,
            ViewElement::Alias(e) => e.label_side = side,
            ViewElement::Group(_) | ViewElement::Link(_) | ViewElement::Cloud(_) => {}
        }
    }
    let mut project = project_of(elements);
    project.dimensions = TestProject::new("dimensions")
        .named_dimension("region", &["north", "south"])
        .build_datamodel()
        .dimensions;
    for variable in &mut project.models[0].variables {
        if !rng.chance(0.3) {
            continue;
        }
        let arrayed = Equation::ApplyToAll(vec!["region".to_string()], "1".to_string());
        match variable {
            Variable::Stock(v) => v.equation = arrayed,
            Variable::Flow(v) => v.equation = arrayed,
            Variable::Aux(v) => v.equation = arrayed,
            Variable::Module(_) => {}
        }
    }
    project
}

/// The two tier arms a generated scene does not reliably hold: a flow whose
/// source end rests on an earlier flow's valve, so the end is drawn above the
/// valve, and an aux whose label hangs over a stock's top edge.
fn tier_scene() -> (datamodel::Project, Vec<Point>) {
    let project = project_of(load(vec![
        flow(1, (100.0, 100.0), &[(0.0, 100.0, 3), (200.0, 100.0, 4)]),
        cloud(3, 1, 0.0, 100.0),
        cloud(4, 1, 200.0, 100.0),
        flow(2, (100.0, 200.0), &[(100.0, 104.0, 1), (100.0, 300.0, 5)]),
        cloud(5, 2, 100.0, 300.0),
        labeled(stock(6, 400.0, 100.0), "bottom"),
        labeled(aux(7, 400.0, 70.0), "bottom"),
    ]));
    (
        project,
        vec![Point::new(100.0, 100.0), Point::new(400.0, 90.0)],
    )
}

/// Elements the grid cannot place: a coordinate that is not finite, one beyond
/// any view, and a group whose edges span too many cells to list.
fn unplaceable_scene() -> datamodel::Project {
    let mut elements = load(vec![
        aux(1, 50.0, 50.0),
        aux(2, f64::NAN, 50.0),
        stock(3, 5e9, 50.0),
    ]);
    elements.push(ViewElement::Group(view_element::Group {
        uid: 4,
        name: "Everything".to_string(),
        x: 0.0,
        y: 0.0,
        width: 1e6,
        height: 1e6,
        is_mdl_view_marker: false,
    }));
    project_of(elements)
}

#[test]
fn a_hit_decided_over_the_elements_within_reach_is_the_hit_decided_over_every_element() {
    let generated = (1..=SEEDS)
        .into_par_iter()
        .map(|seed| {
            let mut rng = Rng::new(seed.wrapping_mul(0x9e37_79b9));
            let strict = crowded(strict_scene(seed).elements, &mut rng);
            let imported = crowded(imported_scene(seed).elements, &mut rng);
            sweep(
                &strict,
                &SCENE,
                &[],
                &mut rng,
                &format!("strict seed {seed}"),
            )
            .merge(sweep(
                &imported,
                &SCENE,
                &[],
                &mut rng,
                &format!("imported seed {seed}"),
            ))
        })
        .reduce(Tally::default, Tally::merge);
    let mut rng = Rng::new(0x71e5);
    let (tiers, tier_points) = tier_scene();
    let tally = generated
        .merge(sweep(&tiers, &SCENE, &tier_points, &mut rng, "tier scene"))
        .merge(sweep(
            &unplaceable_scene(),
            &SCENE,
            &[],
            &mut rng,
            "unplaceable scene",
        ));

    let mut failures = tally.examples.clone();
    if tally.differing > 0 {
        failures.push(format!(
            "{} of {} decisions differ",
            tally.differing, tally.decisions
        ));
    }
    for part in HitPart::ALL {
        if !tally.parts.contains(&part) {
            failures.push(format!("no point was decided on a {part:?}"));
        }
    }
    for rule in Rule::ALL {
        if !tally.rules.contains(&rule) {
            failures.push(format!("no point was decided by {rule:?}"));
        }
    }
    for kind in KINDS {
        if !tally.kinds.contains(kind) {
            failures.push(format!("no point landed on a {kind}"));
        }
    }
    for (answered, how) in [
        (tally.listed, "from the grid"),
        (tally.all, "by walking every element"),
        (tally.everywhere, "with an element listed everywhere"),
    ] {
        if !answered {
            failures.push(format!("no point was answered {how}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_hit_on_world3_decided_over_the_elements_within_reach_is_the_hit_decided_over_every_element() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/metasd/WRLD3-03/wrld3-03.mdl"
    );
    let contents = std::fs::read_to_string(path).expect("the World3 model is in the corpus");
    let project = crate::compat::open_vensim(&contents).expect("World3 opens");
    let tally = sweep(&project, &CORPUS, &[], &mut Rng::new(3), "World3");
    assert!(
        tally.differing == 0,
        "{} of {} decisions differ\n{}",
        tally.differing,
        tally.decisions,
        tally.examples.join("\n")
    );
    assert!(
        tally.listed && tally.rules.len() >= 3,
        "World3 was barely sampled: {} decisions",
        tally.decisions
    );
}
