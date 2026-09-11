// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A stock's inflow and outflow lists reach the salsa inputs as sets.
//!
//! XMILE 1.0 section 4.2 (`docs/reference/xmile-v1.0.html`, "Stocks") calls
//! them "the set of inflows and/or outflows" and lists multiple inflows "in
//! inflow-priority order": each flow is one member with one priority, so a
//! repeated entry names nothing new. The compiler sums each list into the
//! stock's update, and several other readers (conveyor leak validation, the
//! causal graph) iterate them, so a repeat that reached them would integrate
//! the flow twice. `SourceVariableFields::from_datamodel`, the one extraction
//! both the fresh and the incremental sync read, keeps each flow's first
//! occurrence, judged after canonicalization, in list order.

use std::io::BufReader;

use crate::datamodel::{self, SimSpecs};
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
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>dup</name><vendor>x</vendor><product version="1">p</product></header>
  <sim_specs><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="s"><eqn>0</eqn><inflow>f</inflow><inflow>f</inflow></stock>
      <flow name="f"><eqn>1</eqn></flow>
    </variables>
  </model>
</xmile>"#;
    let project = crate::compat::open_xmile(&mut BufReader::new(xml.as_bytes())).expect("imports");
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
