// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The scenario battery: every `ScenarioKind` driven over hand-drawn and
//! imported views through the production patch and sync path, with every
//! finding the audit raises pinned.
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

const FIXTURES: [Fixture; 8] = [
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
];

const REBUILDS_KIND_CHANGE_ELSEWHERE: &str =
    "a variable whose kind changes is rebuilt away from its old element";
const RECREATES_LINKS_OF_REBUILT: &str =
    "the links touching a rebuilt element or flow are re-created";
const REBUILT_FLOW_LANDS_ON_A_SHAPE: &str =
    "a flow rebuilt with a cloud end puts its cloud or valve on another shape";
const DETACHED_CLOUD_INSIDE_STOCK: &str =
    "a detached flow end's cloud is placed inside the stock it left";
const CREATED_FLOW_THROUGH_A_STOCK: &str =
    "a flow created between two stocks is routed through a stock between them";
const CREATED_VALVE_ON_A_SHAPE: &str = "a created flow's valve lands on another shape";
const CHAIN_EXTENSION_ON_A_CLOUD: &str = "a stock added to a drawn chain lands on a cloud";
const DRAWS_OMITTED_CONNECTORS: &str =
    "a sync draws connectors the author left out (whether it should is undecided)";

/// `(fixture, scenario, finding kind, the defect behind it)`.
const KNOWN_DEFECTS: &[(&str, &str, &str, &str)] = &[
    (
        "population",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "population",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "population",
        "delete_middle_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "population",
        "detach_flow",
        "shape_overlap",
        REBUILT_FLOW_LANDS_ON_A_SHAPE,
    ),
    (
        "population",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "logistic_growth",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "logistic_growth",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "logistic_growth",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "logistic_growth_straight",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "logistic_growth_straight",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "logistic_growth_straight",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "fishbanks",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "fishbanks",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "fishbanks",
        "delete_middle_stock",
        "shape_overlap",
        REBUILT_FLOW_LANDS_ON_A_SHAPE,
    ),
    (
        "fishbanks",
        "delete_middle_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "fishbanks",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "fishbanks",
        "extend_chain",
        "shape_overlap",
        CHAIN_EXTENSION_ON_A_CLOUD,
    ),
    (
        "reliability",
        "add_flow_between_stocks",
        "shape_overlap",
        CREATED_VALVE_ON_A_SHAPE,
    ),
    (
        "reliability",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "reliability",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "reliability",
        "delete_middle_stock",
        "shape_overlap",
        REBUILT_FLOW_LANDS_ON_A_SHAPE,
    ),
    (
        "reliability",
        "delete_middle_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "sir",
        "add_flow_between_stocks",
        "pipe_through_stock",
        CREATED_FLOW_THROUGH_A_STOCK,
    ),
    (
        "sir",
        "add_flow_between_stocks",
        "shape_overlap",
        CREATED_FLOW_THROUGH_A_STOCK,
    ),
    (
        "sir",
        "add_then_undo",
        "return_to_original",
        DRAWS_OMITTED_CONNECTORS,
    ),
    (
        "sir",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "sir",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "sir",
        "delete_middle_stock",
        "shape_overlap",
        REBUILT_FLOW_LANDS_ON_A_SHAPE,
    ),
    (
        "sir",
        "delete_middle_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "sir",
        "detach_flow",
        "flow_invariant",
        DETACHED_CLOUD_INSIDE_STOCK,
    ),
    (
        "sir",
        "detach_flow",
        "pipe_through_stock",
        DETACHED_CLOUD_INSIDE_STOCK,
    ),
    (
        "sir",
        "detach_flow",
        "shape_overlap",
        DETACHED_CLOUD_INSIDE_STOCK,
    ),
    (
        "sir",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "sir",
        "restate_variable",
        "return_to_original",
        DRAWS_OMITTED_CONNECTORS,
    ),
    (
        "hares_and_foxes",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "hares_and_foxes",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "lotka_volterra",
        "aux_to_stock",
        "rebuilt_element_moved",
        REBUILDS_KIND_CHANGE_ELSEWHERE,
    ),
    (
        "lotka_volterra",
        "aux_to_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "lotka_volterra",
        "delete_middle_stock",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
    ),
    (
        "lotka_volterra",
        "detach_flow",
        "untouched_link_changed",
        RECREATES_LINKS_OF_REBUILT,
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
