// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The scenario battery: every `ScenarioKind` driven over hand-drawn and
//! imported views through the production patch and sync path, with every
//! finding the audit raises pinned. The imported fixtures are Vensim views
//! with aliases, links the dependency extraction does not explain, variables
//! the author did not draw, and flows that meet at one stock face: the shapes
//! of view a sync meets when an agent edits a published model.
//!
//! `KNOWN_DEFECTS` lists what the sync still gets wrong, one row per (fixture,
//! scenario, finding kind), each naming the defect. The test fails on a finding
//! no row expects and on a row that no longer reproduces, so fixing a defect
//! means deleting its rows, and a regression cannot hide behind a row.

use super::*;
use crate::layout::edit_audit::FindingKind;
use crate::layout::taste::{Degradation, degrade};

struct Fixture {
    key: &'static str,
    path: &'static str,
    /// Start from the shipped view with every connector drawn straight, the way
    /// a modeler who straightens links leaves a diagram, so an edit that
    /// re-curves an untouched link shows up.
    straighten_links: bool,
}

const FIXTURES: [Fixture; 13] = [
    Fixture {
        key: "population",
        path: "default_projects/population/model.xmile",
        straighten_links: false,
    },
    Fixture {
        key: "logistic_growth",
        path: "default_projects/logistic-growth/model.xmile",
        straighten_links: false,
    },
    Fixture {
        key: "logistic_growth_straight",
        path: "default_projects/logistic-growth/model.xmile",
        straighten_links: true,
    },
    Fixture {
        key: "fishbanks",
        path: "default_projects/fishbanks/model.xmile",
        straighten_links: false,
    },
    Fixture {
        key: "reliability",
        path: "default_projects/reliability/model.xmile",
        straighten_links: false,
    },
    Fixture {
        key: "sir",
        path: "test/test-models/samples/SIR/SIR.stmx",
        straighten_links: false,
    },
    Fixture {
        key: "hares_and_foxes",
        path: "test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        straighten_links: false,
    },
    Fixture {
        key: "lotka_volterra",
        path: "test/test-models/samples/Lotka_Volterra/Lotka_Volterra.mdl",
        straighten_links: false,
    },
    Fixture {
        key: "groupon",
        path: "test/metasd/social-network-valuation/groupon 1.mdl",
        straighten_links: false,
    },
    Fixture {
        key: "catastrophe",
        path: "test/metasd/early-warnings-catastrophe/catastropeWarning2.mdl",
        straighten_links: false,
    },
    Fixture {
        key: "beer_game",
        path: "test/metasd/beer-game/RealBeer4-Sterman13.mdl",
        straighten_links: false,
    },
    Fixture {
        key: "bathtub",
        path: "test/metasd/bathtub-statistics/integration3.mdl",
        straighten_links: false,
    },
    Fixture {
        key: "alias1",
        path: "test/alias1/alias1.stmx",
        straighten_links: false,
    },
];

/// A variable the view did not draw is drawn by an edit that does not name it.
const UNDRAWN_VARIABLE_DRAWN: &str = "variable the author left undrawn drawn by an unrelated sync";
/// Flows that met a deleted stock at one point keep their ends there as
/// clouds, on top of each other.
const CLOUDS_MEET_AT_DELETED_STOCK: &str = "clouds of flows through a deleted stock overlap";
/// A created side flow's cloud is kept off other clouds only, and lands on a
/// stock.
const CREATED_CLOUD_ON_SHAPE: &str = "created cloud lands on a shape";

/// A created free-floating element whose footprint the declutter cannot clear
/// (the relaxation jams in a crowded region) stays on another shape.
const CREATED_ELEMENT_ON_SHAPE: &str = "created element left on a shape when relaxation jams";

/// `(fixture, scenario, finding kind, the defect behind it)`.
const KNOWN_DEFECTS: &[(&str, &str, &str, &str)] = &[
    (
        "catastrophe",
        "insert_intermediate",
        "shape_overlap",
        CREATED_ELEMENT_ON_SHAPE,
    ),
    (
        "catastrophe",
        "restate_variable",
        "return_to_original",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_then_undo",
        "return_to_original",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "restate_variable",
        "return_to_original",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "bathtub",
        "add_side_flow",
        "shape_overlap",
        CREATED_CLOUD_ON_SHAPE,
    ),
    (
        "bathtub",
        "delete_middle_stock",
        "shape_overlap",
        CLOUDS_MEET_AT_DELETED_STOCK,
    ),
    (
        "beer_game",
        "add_sector",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "add_sector",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "aux_to_stock",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "aux_to_stock",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "close_loop",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "close_loop",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "delete_parameter",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "delete_parameter",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "insert_intermediate",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "insert_intermediate",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "rename_variable",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "rename_variable",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "restate_variable",
        "shape_overlap",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "beer_game",
        "restate_variable",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_flow_between_stocks",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_parameter",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_sector",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_side_flow",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "add_then_undo",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "aux_to_stock",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "close_loop",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "delete_flow",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "delete_middle_stock",
        "shape_overlap",
        CLOUDS_MEET_AT_DELETED_STOCK,
    ),
    (
        "catastrophe",
        "delete_middle_stock",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "delete_parameter",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "detach_flow",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "extend_chain",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "insert_intermediate",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "rename_by_remove_and_add",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "rename_variable",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
    (
        "catastrophe",
        "restate_variable",
        "unrelated_element_added",
        UNDRAWN_VARIABLE_DRAWN,
    ),
];

fn load(rel: &str) -> datamodel::Project {
    let path = format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"));
    if rel.ends_with(".mdl") {
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        crate::compat::open_vensim(&text).unwrap_or_else(|e| panic!("{path}: {e:?}"))
    } else {
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        crate::compat::open_xmile(&mut std::io::BufReader::new(file))
            .unwrap_or_else(|e| panic!("{path}: {e:?}"))
    }
}

fn fixture(key: &str) -> &'static Fixture {
    FIXTURES
        .iter()
        .find(|f| f.key == key)
        .unwrap_or_else(|| panic!("no fixture {key}"))
}

/// The fixture's project, holding the view its scenarios start from.
fn starting_point(f: &Fixture) -> (datamodel::Project, StockFlow) {
    let mut project = load(f.path);
    let shipped = match project.get_model("main").and_then(|m| m.views.first()) {
        Some(datamodel::View::StockFlow(sf)) => sf.clone(),
        None => panic!("{} ships no view", f.key),
    };
    let view = if f.straighten_links {
        degrade(&shipped, Degradation::StraightenLinks).expect("the view curves some link")
    } else {
        shipped
    };
    project.get_model_mut("main").expect("main").views =
        vec![datamodel::View::StockFlow(view.clone())];
    (project, view)
}

/// Every `(scenario, finding kind)` the battery raises on `f`.
fn battery(f: &Fixture) -> BTreeSet<(&'static str, &'static str)> {
    let (project, view) = starting_point(f);
    let mut found = BTreeSet::new();
    for kind in ScenarioKind::ALL {
        let Some(scenario) = build_scenario(&project, "main", kind) else {
            continue;
        };
        let outcome = run_scenario(&project, "main", &view, &scenario);
        for finding in &outcome.findings {
            eprintln!(
                "{} {} ({}): {} {}: {}",
                f.key,
                kind.name(),
                scenario.description,
                finding.kind.name(),
                finding.subject,
                finding.detail
            );
            found.insert((kind.name(), finding.kind.name()));
        }
    }
    found
}

fn check(key: &str) {
    let f = fixture(key);
    let expected: BTreeSet<(&str, &str)> = KNOWN_DEFECTS
        .iter()
        .filter(|row| row.0 == key)
        .map(|row| (row.1, row.2))
        .collect();
    let actual = battery(f);
    let rows = |set: BTreeSet<&(&str, &str)>| {
        set.into_iter()
            .map(|(s, k)| format!("    (\"{key}\", \"{s}\", \"{k}\", \"\"),"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let unexpected = rows(actual.difference(&expected).collect());
    let fixed = rows(expected.difference(&actual).collect());
    assert!(
        unexpected.is_empty() && fixed.is_empty(),
        "{key}: findings no row expects:\n{unexpected}\nrows that no longer reproduce (delete them):\n{fixed}"
    );
}

#[test]
fn population() {
    check("population");
}

#[test]
fn logistic_growth() {
    check("logistic_growth");
}

#[test]
fn logistic_growth_straight() {
    check("logistic_growth_straight");
}

#[test]
fn fishbanks() {
    check("fishbanks");
}

#[test]
fn reliability() {
    check("reliability");
}

#[test]
fn sir() {
    check("sir");
}

#[test]
fn hares_and_foxes() {
    check("hares_and_foxes");
}

#[test]
fn lotka_volterra() {
    check("lotka_volterra");
}

#[test]
fn groupon() {
    check("groupon");
}

#[test]
fn catastrophe() {
    check("catastrophe");
}

#[test]
fn beer_game() {
    check("beer_game");
}

#[test]
fn bathtub() {
    check("bathtub");
}

#[test]
fn alias1() {
    check("alias1");
}

#[test]
fn every_fixture_has_a_test() {
    // The per-fixture tests above are written out so they run in parallel;
    // this names every fixture key they must cover.
    let tested = [
        "population",
        "logistic_growth",
        "logistic_growth_straight",
        "fishbanks",
        "reliability",
        "sir",
        "hares_and_foxes",
        "lotka_volterra",
        "groupon",
        "catastrophe",
        "beer_game",
        "bathtub",
        "alias1",
    ];
    let keys: Vec<&str> = FIXTURES.iter().map(|f| f.key).collect();
    assert_eq!(keys, tested);
}

#[test]
fn every_scenario_kind_applies_to_some_fixture() {
    let projects: Vec<datamodel::Project> = FIXTURES.iter().map(|f| load(f.path)).collect();
    for kind in ScenarioKind::ALL {
        assert!(
            projects
                .iter()
                .any(|p| build_scenario(p, "main", kind).is_some()),
            "{} applies to no fixture, so the battery never runs it",
            kind.name()
        );
    }
}

#[test]
fn every_known_defect_names_a_fixture_scenario_and_finding() {
    for (key, scenario, finding, defect) in KNOWN_DEFECTS {
        assert!(FIXTURES.iter().any(|f| f.key == *key), "fixture {key}");
        assert!(
            ScenarioKind::ALL.iter().any(|k| k.name() == *scenario),
            "scenario {scenario}"
        );
        assert!(
            FindingKind::ALL.iter().any(|k| k.name() == *finding),
            "finding {finding}"
        );
        assert!(
            !defect.is_empty(),
            "{key} {scenario} {finding} names no defect"
        );
    }
}
