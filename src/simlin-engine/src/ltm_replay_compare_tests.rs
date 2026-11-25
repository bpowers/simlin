// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::test_common::TestProject;
use crate::testutils::{x_aux, x_flow, x_model, x_module_named, x_stock};

fn compared(project: &TestProject) -> Comparison {
    let found = compare(&project.build_datamodel()).expect("the fixture is compared");
    assert!(found.misreproduced.is_empty(), "{:?}", found.misreproduced);
    found
}

fn links(found: &[OneSided]) -> Vec<String> {
    found
        .iter()
        .map(|link| format!("{} -> {}", link.from, link.to))
        .collect()
}

fn scalar() -> TestProject {
    TestProject::new("scalar")
        .with_sim_time(0.0, 6.0, 1.0)
        .stock("s", "10", &["f"], &[], None)
}

/// An arrayed stock whose elements grow at rates of their own.
fn arrayed() -> TestProject {
    TestProject::new("arrayed")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("R", &["a", "b", "c"])
        .named_dimension("P", &["x", "y"])
        .array_with_ranges("k[R]", vec![("a", "0.1"), ("b", "0.2"), ("c", "0.3")])
        .array_stock("pop[R]", "100 + 10 * R", &["grow"], &[], None)
}

/// [`arrayed`] with a second stock over both dimensions, on a loop through
/// `t[R]`.
fn two_dimensions() -> TestProject {
    arrayed()
        .array_stock("m[R,P]", "10 + 3 * R + P", &["mg"], &[], None)
        .array_flow("mg[R,P]", "k[R] * m[R,P] * (1 - t[R] / 1000)", None)
        .array_flow("grow[R]", "k[R] * pop[R]", None)
}

#[test]
fn the_scores_of_ordinary_models_agree() {
    // Each by how many of its scores are not 0 throughout, so an agreement of
    // zeros is not one.
    let rows = [
        (
            "growth to a limit",
            scalar().flow("f", "s * 0.1 * (1 - s / 100)", None),
            2,
        ),
        (
            "a smoothed input",
            scalar()
                .aux("seen", "SMTH1(s, 3)", None)
                .flow("f", "0.1 * (100 - seen)", None),
            7,
        ),
        (
            "a condition",
            scalar()
                .aux("limit", "MIN(s, 12) ^ 2 + SQRT(s)", None)
                .flow("f", "IF limit > 140 THEN 1 ELSE limit / 100", None),
            3,
        ),
        (
            "elements of their own",
            arrayed().array_flow("grow[R]", "k[R] * pop[R] * (1 - pop[R] / 1000)", None),
            6,
        ),
        (
            "a share of a sum",
            arrayed()
                .array_aux("share[R]", "pop[R] / SUM(pop[*])")
                .array_flow("grow[R]", "k[R] * pop[R] * (1 - share[R])", None),
            18,
        ),
        (
            "a sum spread over an array",
            arrayed().array_aux("total[R]", "SUM(pop[*])").array_flow(
                "grow[R]",
                "k[R] * pop[R] * (1 - total[R] / 2000)",
                None,
            ),
            15,
        ),
        (
            "the largest of two dimensions spread over one",
            two_dimensions().array_aux("t[R]", "MAX(m[*,*])"),
            30,
        ),
        (
            "a row's sum",
            two_dimensions().array_aux("t[R]", "SUM(m[R,*])"),
            30,
        ),
    ];
    for (name, project, active) in rows {
        let found = compared(&project);
        assert_eq!(
            (
                found.differences.len(),
                found.not_read.len(),
                found.not_slots.len(),
                found.not_scored.len()
            ),
            (0, 0, 0, 0),
            "{name}: {found:#?}"
        );
        assert_eq!(found.agreed_active, active, "{name}");
    }
}

/// `s -> m`, of which the model reads one output and another model another.
fn pathways_project() -> datamodel::Project {
    let model = |name: &str, variables| x_model(name, variables);
    let mut project = scalar().build_datamodel();
    project.models = vec![
        model(
            "main",
            vec![
                x_stock("s", "100", &["f"], &[], None),
                x_module_named("m", "inner", &[("s", "m.input_val")], None),
                x_flow("f", "m.pos * 0.1", None),
            ],
        ),
        model(
            "other",
            vec![
                x_aux("driver", "5", None),
                x_module_named("n", "inner", &[("driver", "n.input_val")], None),
                x_aux("sink", "n.neg", None),
            ],
        ),
        model(
            "inner",
            vec![
                x_aux("input_val", "0", None),
                x_aux("pos", "input_val * 0.02", None),
                x_aux("neg", "0 - input_val", None),
            ],
        ),
    ];
    project
}

/// A model in which a link's generated score is off for a reason, and the
/// link.
fn differing(why: Why) -> (datamodel::Project, &'static str) {
    let (project, link) = match why {
        // Millions added to millionths.
        Why::Rounding => (
            arrayed()
                .array_aux("x[R]", "1e6 * R + pop[R] * 1e-8")
                .aux("t", "SUM(x[*])", None)
                .array_flow("grow[R]", "k[R] * t * 1e-6", None),
            "x[a] -> t",
        ),
        Why::LiveLags => (
            scalar().flow("f", "PREVIOUS(s) * 0.1 + s * 0.01", None),
            "s -> f",
        ),
        Why::LiveClock => (scalar().flow("f", "1 + STEP(0.1 * s, 2)", None), "s -> f"),
        Why::ChangedLast => (
            arrayed()
                .aux("scale", "pop[a] / 100", None)
                .aux("t", "1 + SUM(pop[*] * scale)", None)
                .array_flow("grow[R]", "k[R] * t / 100", None),
            "scale -> $\u{205A}ltm\u{205A}agg\u{205A}0",
        ),
        Why::OneRead => (
            arrayed().array_flow("grow[R]", "k[R] * (pop[R] + pop[a])", None),
            "pop[a] -> grow[a]",
        ),
        Why::WholeChange => (
            two_dimensions()
                .array_with_ranges("w[P]", vec![("x", "pop[a]"), ("y", "pop[b]")])
                .array_aux("t[R]", "SUM(m[R,*] * w[*]) / 100"),
            "m[a,x] -> t[a]",
        ),
        Why::Zero => (
            arrayed()
                .aux("v", "SUM(pop[*])", None)
                .array_flow("grow[R]", "v * R * 0.001", None),
            "v -> grow[a]",
        ),
        Why::WholeSource => (
            arrayed()
                .array_aux("w[R]", "pop[R] / 100")
                .array_aux("t[R]", "1 + SUM(pop * w[*]) / 1000")
                .array_flow("grow[R]", "k[R] * t[R]", None),
            "w[a] -> t[a]",
        ),
        Why::OtherPathway => return (pathways_project(), "s -> m"),
        Why::Unexplained => (
            two_dimensions().array_aux("t[R]", "SUM(m[R,*] * m[R,x])"),
            "m[a,x] -> t[a]",
        ),
    };
    (project.build_datamodel(), link)
}

#[test]
fn each_way_a_score_is_off_is_named() {
    for why in Why::ALL {
        let (project, link) = differing(why);
        let found = compare(&project).expect("the fixture is compared");
        let of_the_link: Vec<Why> = found
            .differences
            .iter()
            .filter(|d| format!("{} -> {}", d.from, d.to) == link)
            .map(|d| d.why)
            .collect();
        assert_eq!(of_the_link, [why], "{link}: {found:#?}");
        assert_eq!(
            found.counts().get(&why).copied(),
            Some(found.differences.iter().filter(|d| d.why == why).count())
        );
    }
}

#[test]
fn a_difference_holds_the_numbers_it_is_of() {
    let (project, _) = differing(Why::OneRead);
    let found = compare(&project).expect("compared");
    let [difference] = found.differences.as_slice() else {
        panic!("{found:#?}");
    };
    // `grow[a]` is 2 * k * pop[a], half of it through each read.
    assert!((difference.generated - 0.5).abs() < 1e-9, "{difference:?}");
    assert!((difference.replayed - 1.0).abs() < 1e-9, "{difference:?}");
    assert!(difference.row > 0);
}

#[test]
fn a_link_one_of_the_two_does_not_score_is_said_to_be() {
    // A read of an element the run picks is scored as a read of the array.
    let found = compared(
        &arrayed()
            .aux("i", "1 + INT(pop[a]) MOD 3", None)
            .aux("picked", "pop[i]", None)
            .array_flow("grow[R]", "k[R] * picked", None),
    );
    assert_eq!(links(&found.not_slots), ["pop -> picked"]);
    assert_eq!(
        links(&found.not_scored),
        ["pop[a] -> picked", "pop[b] -> picked", "pop[c] -> picked"]
    );

    // A sum over a subrange is scored from the elements outside it as well.
    let found = compared(
        &TestProject::new("subrange")
            .with_sim_time(0.0, 6.0, 1.0)
            .named_dimension("R", &["a", "b"])
            .named_dimension("P", &["x", "y", "z"])
            .named_dimension("S", &["x", "y"])
            .array_stock("m[R,P]", "10 + 3 * R + P * P", &["mg"], &[], None)
            .array_flow("mg[R,P]", "m[R,P] * 0.1 * P * (1 - t[R] / 1000)", None)
            .array_aux("t[R]", "1 + SUM(m[R, *:S])"),
    );
    assert_eq!(links(&found.not_read), ["m[a,z] -> t[a]", "m[b,z] -> t[b]"]);
}

#[test]
fn a_model_that_does_not_run_is_not_one_whose_comparison_is_broken() {
    let no_model = datamodel::Project {
        models: vec![],
        ..scalar().build_datamodel()
    };
    assert_eq!(
        compare(&no_model).unwrap_err(),
        NotCompared::DoesNotRun("no model".to_string())
    );
    let uncompiled = scalar().flow("f", "missing * 2", None);
    let no_steps = scalar().flow("f", "1", None).with_sim_time(0.0, 6.0, 0.0);
    for project in [uncompiled, no_steps] {
        let why = compare(&project.build_datamodel()).unwrap_err();
        assert!(matches!(why, NotCompared::DoesNotRun(_)), "{why:?}");
    }
    let why = NotCompared::DoesNotRun("a reason".to_string()).to_string();
    assert_eq!(why, "does not run: a reason");
    let why = NotCompared::Broken("a reason".to_string()).to_string();
    assert_eq!(why, "broken: a reason");
}

/// What makes a comparison broken, as against a model that has none: the two
/// runs are not of one model.
#[test]
fn a_run_with_scores_that_is_not_the_models_run_is_broken() {
    let project = scalar().flow("f", "s * 0.1", None).build_datamodel();
    let other = scalar().flow("f", "s * 0.2", None).build_datamodel();
    let longer = scalar()
        .flow("f", "s * 0.1", None)
        .with_sim_time(0.0, 7.0, 1.0)
        .build_datamodel();
    let more = scalar()
        .flow("f", "s * 0.1", None)
        .aux("extra", "TIME", None)
        .build_datamodel();
    let plain = plain_run(&more, "main").expect("runs");
    let of_more = Side::new(&plain, Reducers::Inline).expect("cut").replay;
    let scored = |project| Generated::new(project, "main", of_more.instances()).expect("runs");
    assert!(
        scored(&project)
            .ran_the_same_model(&plain, &of_more)
            .is_err_and(|e| e.contains("has no extra"))
    );
    let plain = plain_run(&project, "main").expect("runs");
    let replay = Side::new(&plain, Reducers::Inline).expect("cut").replay;
    assert!(scored(&project).ran_the_same_model(&plain, &replay).is_ok());
    assert!(
        scored(&other)
            .ran_the_same_model(&plain, &replay)
            .is_err_and(|e| e.contains("differs between the two runs"))
    );
    assert!(
        scored(&longer)
            .ran_the_same_model(&plain, &replay)
            .is_err_and(|e| e.contains("different rows"))
    );
}

#[test]
fn a_model_without_state_is_not_one_whose_links_go_unscored() {
    let found = compared(
        &TestProject::new("stateless")
            .with_sim_time(0.0, 6.0, 1.0)
            .aux("x", "TIME * TIME", None)
            .aux("y", "x + 1", None),
    );
    assert_eq!((found.agreed, found.not_scored.len()), (0, 0));
}

#[test]
fn a_link_between_what_is_never_a_number_is_not_one_that_agrees() {
    let found = compared(
        &scalar()
            .aux("never", "s / 0 - s / 0", None)
            .aux("of_it", "never * 2", None)
            .flow("f", "s * 0.1 + 0 * of_it", None),
    );
    assert_eq!(
        (found.agreed, found.differences.len()),
        (0, 0),
        "{found:#?}"
    );
}

#[test]
fn an_array_of_one_element_is_compared_like_any_other() {
    // Its one slot has no element in its results name, and the generated links
    // name the element.
    let found = compared(
        &TestProject::new("one_element")
            .with_sim_time(0.0, 6.0, 1.0)
            .indexed_dimension("D", 1)
            .array_stock("pop[D]", "100", &["grow"], &[], None)
            .array_flow("grow[D]", "pop[D] * 0.1", None),
    );
    assert_eq!(
        (
            found.agreed_active,
            found.not_slots.len(),
            found.not_scored.len()
        ),
        (2, 0, 0),
        "{found:#?}"
    );
}

#[test]
fn a_target_that_is_never_a_number_is_not_one_whose_links_go_unscored() {
    let found = compared(
        &arrayed()
            .array_aux("on[R]", "0")
            .aux("selected", "VECTOR SELECT(on[*], pop[*], NAN, 0, 0)", None)
            .array_flow("grow[R]", "k[R] * pop[R]", None),
    );
    assert_eq!(links(&found.not_scored), Vec::<String>::new());
}
