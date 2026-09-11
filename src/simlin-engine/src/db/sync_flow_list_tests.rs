// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A stock's inflow and outflow lists are read as sets
//! (`datamodel::distinct_stock_flows`): each flow counts once, at its first
//! occurrence after canonicalization. This is the engine's rule, unverified
//! against Stella or Vensim; XMILE 1.0 section 4.2 never addresses a repeated
//! tag (the owner's rustdoc quotes what it does say).
//!
//! A file can repeat a flow, and the readers store what it says, so every
//! reader that decides what the model does reads the set. Rows, one per such
//! reader:
//!
//! - the salsa sync, for an ordinary stock (`SourceVariableFields::from_datamodel`)
//! - the special-stock build, for a queue and a conveyor, each list
//!   (`queue_compile::build_compiled`, before either expansion)
//! - the layout's stock-flow metadata (`layout::compute_metadata`)
//! - the MDL writer's `INTEG` (`mdl::writer::write_stock_variable`)
//! - the patch ops, which store the set (`patch::tests`)
//!
//! and the advisory the sync's record of the repeat raises, once per stock
//! (`RepeatedStockFlow`; exactly once across reaches and revisions in
//! `db::diagnostic_payload_tests::every_warning_family_is_emitted_once_across_revisions`).

use std::io::BufReader;

use crate::common::{ErrorCode, Ident};
use crate::datamodel::{self, SimSpecs};
use crate::diagnostic::{DiagnosticCategory, DiagnosticSeverity};
use crate::test_common::TestProject;

fn three_steps() -> SimSpecs {
    SimSpecs {
        start: 0.0,
        stop: 3.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: Some(datamodel::Dt::Dt(1.0)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Month".to_string()),
    }
}

fn parse(xml: &str) -> datamodel::Project {
    crate::compat::open_xmile(&mut BufReader::new(xml.as_bytes())).expect("imports")
}

#[test]
fn distinct_stock_flows_keeps_first_occurrences_and_names_repeats() {
    let flows: Vec<String> = ["b", "Flow A", "b", "flow_a", "c", "FLOW_A"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let set = datamodel::distinct_stock_flows(&flows);
    assert_eq!(set.flows, vec!["b", "Flow A", "c"]);
    assert_eq!(set.repeated, vec!["b", "flow_a"]);

    let no_repeat = datamodel::distinct_stock_flows(&["x".to_string(), "y".to_string()]);
    assert_eq!(no_repeat.flows, vec!["x", "y"]);
    assert!(no_repeat.repeated.is_empty());
}

#[test]
fn a_duplicated_stock_flow_integrates_once() {
    TestProject::new_with_specs("dup", three_steps())
        .flow("f", "1", None)
        .flow("g", "0.5", None)
        .stock("s", "0", &["f", "f"], &["g", "g"], None)
        .assert_vm_result("s", &[0.0, 0.5, 1.0, 1.5]);
}

#[test]
fn spellings_of_one_flow_are_one_member() {
    TestProject::new_with_specs("dup", three_steps())
        .flow("flow_a", "1", None)
        .stock("s", "0", &["flow_a", "Flow A", "FLOW_A"], &[], None)
        .assert_vm_result("s", &[0.0, 1.0, 2.0, 3.0]);
}

/// A file carrying a repeated `<inflow>` imports with the repeat in the
/// datamodel (the reader stores what the file says), and simulates with the
/// flow integrated once.
#[test]
fn a_file_with_a_repeated_inflow_integrates_it_once() {
    let project = parse(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>dup</name><vendor>x</vendor><product version="1">p</product></header>
  <sim_specs><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="s"><eqn>0</eqn><inflow>f</inflow><inflow>f</inflow></stock>
      <flow name="f"><eqn>1</eqn></flow>
    </variables>
  </model>
</xmile>"#,
    );
    let stock = project.models[0]
        .variables
        .iter()
        .find_map(|v| match v {
            datamodel::Variable::Stock(s) => Some(s),
            _ => None,
        })
        .expect("stock s");
    assert_eq!(
        stock.inflows,
        vec!["f".to_string(), "f".to_string()],
        "fixture: the datamodel carries the file's repeat"
    );
    TestProject::from_datamodel(project).assert_vm_result("s", &[0.0, 1.0, 2.0, 3.0]);
}

/// Each named series of `project` through `queue_compile::build_sim`, the
/// dispatch every VM-backed production caller funnels through, or the refusal.
fn special_stock_series(
    project: &datamodel::Project,
    names: &[&str],
) -> Result<Vec<Vec<f64>>, String> {
    let mut db = crate::db::SimlinDb::default();
    let source_project = db.sync(project);
    let main = project.models[0].name.clone();
    let mut vm = crate::queue_compile::build_sim(
        &mut db,
        source_project,
        project,
        &main,
        crate::db::LtmOverlay::Off,
    )
    .map_err(|e| format!("{e:?}"))?;
    vm.run_to_end().map_err(|e| format!("{e:?}"))?;
    names
        .iter()
        .map(|n| {
            vm.get_series(&Ident::new(n))
                .ok_or_else(|| format!("no series {n}"))
        })
        .collect()
}

/// Rows: a queue's inflow, a queue's secondary outflow, a conveyor's inflow,
/// a conveyor's outflow. Each file with the tag repeated simulates exactly as
/// the file without the repeat. Without the set `build_compiled` takes, the
/// queue inflow and both conveyor rows fail (the conveyor outflow is refused
/// as a second primary outflow); the queue's secondary outflow is served off
/// the synced inputs and holds either way, so it pins the rule, not the build.
#[test]
fn a_special_stock_whose_file_repeats_a_flow_integrates_it_once() {
    const QUEUE: &str = include_str!("../../../../test/queues/minimal_queue.xmile");
    const CONVEYOR: &str = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let rows: [(&str, &str, &str, &[&str]); 4] = [
        (
            "queue inflow",
            QUEUE,
            "<inflow>arrivals</inflow>",
            &["waiting", "served"],
        ),
        (
            "queue secondary outflow",
            QUEUE,
            "<outflow>balk</outflow>",
            &["waiting", "served"],
        ),
        (
            "conveyor inflow",
            CONVEYOR,
            "<inflow>matriculating</inflow>",
            &["students", "alumni"],
        ),
        (
            "conveyor outflow",
            CONVEYOR,
            "<outflow>graduating</outflow>",
            &["students", "alumni"],
        ),
    ];
    let mut differ = Vec::new();
    for (label, file, tag, names) in rows {
        assert_eq!(
            file.matches(tag).count(),
            1,
            "{label}: fixture names the tag once"
        );
        let repeated = file.replacen(tag, &format!("{tag}{tag}"), 1);
        if special_stock_series(&parse(&repeated), names)
            != special_stock_series(&parse(file), names)
        {
            differ.push(label);
        }
    }
    assert_eq!(
        differ,
        Vec::<&str>::new(),
        "a repeated tag must simulate as the file without it"
    );
}

#[test]
fn a_stock_list_that_repeats_a_flow_warns_once_naming_the_repeats() {
    let project = TestProject::new("repeat")
        .flow("f", "1", None)
        .flow("g", "1", None)
        .stock("s", "0", &["f", "F", "f"], &["g", "g"], None)
        .stock("t", "0", &["f"], &[], None)
        .build_datamodel();
    let mut db = crate::db::SimlinDb::default();
    let source_project = db.sync(&project);
    let diags = crate::db::collect_all_diagnostics(&db, source_project, crate::db::LtmOverlay::Off);
    let warnings: Vec<_> = diags
        .iter()
        .filter(|d| d.is(DiagnosticCategory::Model, ErrorCode::RepeatedStockFlow))
        .collect();
    assert_eq!(warnings.len(), 1, "one warning, for s only: {diags:?}");
    let w = warnings[0];
    assert_eq!(w.severity, DiagnosticSeverity::Warning);
    assert_eq!(w.variable.as_deref(), Some("s"));
    let reason = w.reason().unwrap_or_default();
    assert!(
        reason.contains("inflow list repeats 'f'") && reason.contains("outflow list repeats 'g'"),
        "names both lists' repeats: {reason}"
    );
}

#[test]
fn layout_metadata_reads_a_repeated_flow_once() {
    let project = TestProject::new("repeat")
        .flow("f", "1", None)
        .stock("s", "0", &["f", "f"], &[], None)
        .build_datamodel();
    let name = project.models[0].name.clone();
    let metadata = crate::layout::compute_metadata(&project, &name, None).expect("metadata");
    assert_eq!(metadata.stock_to_inflows["s"], vec!["f".to_string()]);
}

#[test]
fn mdl_export_writes_a_repeated_flow_once() {
    let project = TestProject::new("repeat")
        .flow("f", "1", None)
        .stock("s", "0", &["f", "f"], &[], None)
        .build_datamodel();
    let mdl = crate::compat::to_mdl(&project).expect("exports");
    assert!(
        mdl.contains("INTEG") && !mdl.contains("f+f") && !mdl.contains("f + f"),
        "the INTEG names f once:\n{mdl}"
    );
}
