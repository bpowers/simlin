// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A patch keeps the order of a stock's inflow and outflow lists. The order
//! is the flows' priority (XMILE 1.0 section 4.2): a queue serves its
//! outflows in list order and may mark only a later one `<overflow/>`, and a
//! conveyor admits its coupled inflows in list order.

use std::collections::HashMap;
use std::io::BufReader;

use super::*;
use crate::patch_sharing_tests::{MODEL_OPERATION_VARIANTS, model_operation_variant};
use crate::test_common::TestProject;

fn model_patch(ops: Vec<ModelOperation>) -> ProjectPatch {
    ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops,
        }],
    }
}

fn open(xmile: &str) -> datamodel::Project {
    crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("the fixture opens")
}

/// Every series the project's run saves, by name.
fn series(project: &datamodel::Project) -> HashMap<String, Vec<f64>> {
    let main = project.models[0].name.clone();
    let mut vm = crate::queue_compile::build_vm(project, &main).expect("the fixture builds");
    vm.run_to_end().expect("the fixture runs");
    crate::test_common::collect_results(&vm.into_results())
}

fn renamed(project: &datamodel::Project, from: &str, to: &str) -> datamodel::Project {
    let mut next = project.clone();
    apply_patch(
        &mut next,
        model_patch(vec![ModelOperation::RenameVariable {
            from: from.to_string(),
            to: to.to_string(),
        }]),
    )
    .expect("the rename applies");
    next
}

/// A stock whose lists are in no alphabetical order, the flows it names, a
/// flow it does not, and an empty view for the view operations.
fn tank() -> datamodel::Project {
    let mut project = TestProject::new("order")
        .flow("zin", "1", None)
        .flow("ain", "1", None)
        .flow("zeta", "1", None)
        .flow("mid", "1", None)
        .flow("alpha", "1", None)
        .flow("unrelated", "1", None)
        .aux("level", "tank + 1", None)
        .stock(
            "tank",
            "10",
            &["zin", "ain"],
            &["zeta", "mid", "alpha"],
            None,
        )
        .stock("elsewhere", "0", &["unrelated"], &[], None)
        .build_datamodel();
    project.models[0].views.push(empty_view());
    project
}

fn empty_view() -> datamodel::View {
    datamodel::View::StockFlow(datamodel::StockFlow {
        name: None,
        elements: vec![].into(),
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })
}

fn tank_stock(project: &datamodel::Project) -> datamodel::Stock {
    match project.models[0].get_variable("tank") {
        Some(Variable::Stock(stock)) => stock.clone(),
        _ => unreachable!("the fixture's tank is a stock"),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

#[test]
fn every_operation_leaves_a_stocks_flows_in_the_order_they_were_given() {
    let base = tank();
    let stock = tank_stock(&base);
    let (inflows, outflows) = (names(&["zin", "ain"]), names(&["zeta", "mid", "alpha"]));
    let Some(Variable::Flow(mid)) = base.models[0].get_variable("mid").cloned() else {
        unreachable!("the fixture's mid is a flow")
    };
    let Some(Variable::Aux(level)) = base.models[0].get_variable("level").cloned() else {
        unreachable!("the fixture's level is an aux")
    };
    let rename = |from: &str, to: &str| ModelOperation::RenameVariable {
        from: from.to_string(),
        to: to.to_string(),
    };
    // One row per operation that can reach a stock's lists, and one per
    // operation that cannot, each with the lists the stock holds after it.
    // `model_operation_variant` matches without a wildcard, so a variant added
    // to `ModelOperation` fails the last assertion until it has a row.
    let rows: Vec<(&str, ModelOperation, Vec<String>, Vec<String>)> = vec![
        (
            "an upsert of the stock with its lists as they are",
            ModelOperation::UpsertStock(datamodel::Stock {
                documentation: "edited".to_string(),
                ..stock.clone()
            }),
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "an upsert of the stock with lists in another order",
            ModelOperation::UpsertStock(datamodel::Stock {
                inflows: names(&["ain", "zin"]),
                outflows: names(&["mid", "zeta", "alpha"]),
                ..stock.clone()
            }),
            names(&["ain", "zin"]),
            names(&["mid", "zeta", "alpha"]),
        ),
        (
            "an upsert of a flow the stock names",
            ModelOperation::UpsertFlow(datamodel::Flow {
                equation: datamodel::Equation::Scalar("2".to_string()),
                ..mid
            }),
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "an upsert of an aux",
            ModelOperation::UpsertAux(datamodel::Aux {
                equation: datamodel::Equation::Scalar("tank + 2".to_string()),
                ..level
            }),
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "an upsert of a module",
            ModelOperation::UpsertModule(datamodel::Module {
                ident: "instance".to_string(),
                model_name: "main".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![],
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "a delete of a flow in the middle of a list",
            ModelOperation::DeleteVariable {
                ident: "mid".to_string(),
            },
            inflows.clone(),
            names(&["zeta", "alpha"]),
        ),
        (
            "a rename of a flow the stock does not name",
            rename("unrelated", "still unrelated"),
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "a rename of the first flow of a list to a name that sorts last",
            rename("ain", "zz in"),
            names(&["zin", "zz_in"]),
            outflows.clone(),
        ),
        (
            "a rename of the first outflow to a name that sorts between the others",
            rename("zeta", "beta"),
            inflows.clone(),
            names(&["beta", "mid", "alpha"]),
        ),
        (
            "an upsert of a view",
            ModelOperation::UpsertView {
                index: 0,
                view: empty_view(),
            },
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "a delete of a view",
            ModelOperation::DeleteView { index: 0 },
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "an update of the lists",
            ModelOperation::UpdateStockFlows {
                ident: "tank".to_string(),
                inflows: names(&["zin", "Ain", "ain"]),
                outflows: names(&["alpha", "zeta"]),
            },
            names(&["zin", "ain"]),
            names(&["alpha", "zeta"]),
        ),
        (
            "a named loop",
            ModelOperation::SetLoopName {
                variables: names(&["tank", "zeta"]),
                name: "drain".to_string(),
                description: None,
            },
            inflows.clone(),
            outflows.clone(),
        ),
        (
            "a view edit",
            ModelOperation::EditView {
                index: 0,
                upsert: vec![],
                remove: vec![],
            },
            inflows.clone(),
            outflows.clone(),
        ),
    ];

    let mut covered = [false; MODEL_OPERATION_VARIANTS];
    for (what, op, want_inflows, want_outflows) in rows {
        covered[model_operation_variant(&op)] = true;
        let mut project = base.clone();
        apply_patch(&mut project, model_patch(vec![op]))
            .unwrap_or_else(|err| panic!("{what}: {err:?}"));
        let after = tank_stock(&project);
        assert_eq!(after.inflows, want_inflows, "{what}: inflows");
        assert_eq!(after.outflows, want_outflows, "{what}: outflows");
    }
    assert_eq!(
        covered, [true; MODEL_OPERATION_VARIANTS],
        "every operation has a row"
    );
}

/// A queue feeding a capacity-limited conveyor, with an `<overflow/>` second
/// outflow. `balk` sorts before `into_belt`, and an overflow may never be a
/// queue's first outflow (`ErrorCode::QueueOverflowNotOnQueue`).
const QUEUE_WITH_OVERFLOW: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>overflow</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>balk</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="balk"><eqn>0</eqn><overflow/></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="balked"><eqn>0</eqn><inflow>balk</inflow></stock>
  </variables></model>
</xmile>"#;

/// Two queues feeding one conveyor whose shared budget admits the first
/// listed inflow first; the belt lists `into_belt_b` before `into_belt_a`.
const TWO_QUEUES_ONE_CONVEYOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>two queues</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting_a"><eqn>0</eqn><inflow>arrivals_a</inflow><outflow>into_belt_a</outflow><queue/></stock>
    <flow name="arrivals_a"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt_a"><eqn>0</eqn></flow>
    <stock name="waiting_b"><eqn>0</eqn><inflow>arrivals_b</inflow><outflow>into_belt_b</outflow><queue/></stock>
    <flow name="arrivals_b"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt_b"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn>
      <inflow>into_belt_b</inflow><inflow>into_belt_a</inflow>
      <outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><in_limit>6</in_limit></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <flow name="unrelated"><eqn>1</eqn></flow>
    <stock name="elsewhere"><eqn>0</eqn><inflow>unrelated</inflow></stock>
  </variables></model>
</xmile>"#;

/// One queue with two ordinary outflows, the first listed serving everything;
/// `zeta` is listed before `alpha`.
const COMPETING_OUTFLOWS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>competing</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>zeta</outflow><outflow>alpha</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="zeta"><eqn>0</eqn></flow>
    <flow name="alpha"><eqn>0</eqn></flow>
    <stock name="via_zeta"><eqn>0</eqn><inflow>zeta</inflow></stock>
    <stock name="via_alpha"><eqn>0</eqn><inflow>alpha</inflow></stock>
    <flow name="unrelated"><eqn>1</eqn></flow>
    <stock name="elsewhere"><eqn>0</eqn><inflow>unrelated</inflow></stock>
  </variables></model>
</xmile>"#;

#[test]
fn a_queue_whose_overflow_sorts_first_still_expands_after_an_edit() {
    let project = open(QUEUE_WITH_OVERFLOW);
    let expands = |project: &datamodel::Project| {
        crate::queue_compile::expand_queues(project, &project.models[0].name).map(|_| ())
    };
    assert_eq!(expands(&project), Ok(()), "the fixture expands as opened");

    assert_eq!(
        expands(&renamed(&project, "graduating", "leaving")),
        Ok(()),
        "after a rename of a flow the queue does not name"
    );

    let Some(Variable::Stock(waiting)) = project.models[0].get_variable("waiting").cloned() else {
        unreachable!("the fixture's waiting is a stock")
    };
    let mut upserted = project.clone();
    apply_patch(
        &mut upserted,
        model_patch(vec![ModelOperation::UpsertStock(datamodel::Stock {
            documentation: "edited".to_string(),
            ..waiting
        })]),
    )
    .unwrap();
    assert_eq!(expands(&upserted), Ok(()), "after an upsert of the queue");
}

#[test]
fn a_rename_of_an_unrelated_flow_changes_no_series_where_order_is_priority() {
    for (what, xmile, reordered) in [
        (
            "two queues feeding one conveyor",
            TWO_QUEUES_ONE_CONVEYOR,
            "waiting_a",
        ),
        ("two outflows of one queue", COMPETING_OUTFLOWS, "via_zeta"),
    ] {
        let project = open(xmile);
        let before = series(&project);
        let mut after = series(&renamed(&project, "unrelated", "still unrelated"));
        // The renamed flow's own series moves to its new name.
        let moved = after.remove("still_unrelated");
        assert!(moved.is_some(), "{what}: the renamed flow has a series");
        after.insert("unrelated".to_string(), moved.unwrap_or_default());
        assert!(
            before[reordered].iter().any(|value| *value != 0.0),
            "{what}: the fixture's priority shows in {reordered}"
        );
        assert_eq!(before.len(), after.len(), "{what}: the same variables");
        for (name, values) in &before {
            assert_eq!(Some(values), after.get(name), "{what}: {name}");
        }
    }
}
