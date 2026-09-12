// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of a view edit's model operations (`derived_operations`, applied by
//! `ModelOperation::EditView`) and of the view edits a delete and a rename
//! imply.
//!
//! Every edit is applied through the production patch path and the committed
//! project checked for model/view agreement. The tables pin each operation
//! family -- creates derived from `VariableKind::ALL`, renames, deletes, stock
//! list deltas in both directions -- and each conflict, which refuses the whole
//! patch.

use crate::editing::test_support::{
    agreement_report, alias, apply_edit, aux, cloud, edit_patch, flow, link, load, main_model,
    project_of, stock, stock_flow_of, view_of,
};
use crate::json;
use crate::patch::is_view_only_patch;

use super::*;

/// Two stocks joined by a flow, a cloud-to-cloud flow, an aux linked to that
/// flow, and an alias of the aux.
fn scene() -> datamodel::Project {
    project_of(load(vec![
        stock(1, 100.0, 100.0),
        stock(2, 400.0, 100.0),
        flow(3, (250.0, 100.0), &[(122.5, 100.0, 1), (377.5, 100.0, 2)]),
        flow(4, (200.0, 300.0), &[(100.0, 300.0, 5), (300.0, 300.0, 6)]),
        cloud(5, 4, 100.0, 300.0),
        cloud(6, 4, 300.0, 300.0),
        aux(7, 250.0, 450.0),
        link(8, 7, 4, None),
        alias(9, 7, 500.0, 450.0),
    ]))
}

fn describe(op: &ModelOperation) -> String {
    match op {
        ModelOperation::UpsertStock(s) => format!("upsert stock {}", s.ident),
        ModelOperation::UpsertFlow(f) => format!("upsert flow {}", f.ident),
        ModelOperation::UpsertAux(a) => format!("upsert aux {}", a.ident),
        ModelOperation::UpsertModule(m) => format!("upsert module {}", m.ident),
        ModelOperation::DeleteVariable { ident } => format!("delete {ident}"),
        ModelOperation::RenameVariable { from, to } => format!("rename {from} -> {to}"),
        ModelOperation::UpdateStockFlows {
            ident,
            inflows,
            outflows,
        } => format!("stock {ident} in {inflows:?} out {outflows:?}"),
        ModelOperation::UpsertView { .. } => "upsert view".to_string(),
        ModelOperation::DeleteView { .. } => "delete view".to_string(),
        ModelOperation::SetLoopName { .. } => "set loop name".to_string(),
        ModelOperation::EditView { .. } => "edit view".to_string(),
    }
}

/// The operations `edit` implies against `project`, described, or the error.
fn operations(
    project: &datamodel::Project,
    edit: &ViewEdit,
) -> std::result::Result<Vec<String>, String> {
    let base = stock_flow_of(project);
    let next = edited_view(base, &edit.upsert, &edit.remove);
    derived_operations(main_model(project), base, &next)
        .map(|ops| ops.iter().map(describe).collect())
        .map_err(|e| e.to_string())
}

fn upsert(elements: Vec<json::ViewElement>) -> ViewEdit {
    ViewEdit {
        upsert: load(elements),
        remove: Vec::new(),
    }
}

fn named_aux(uid: i32, name: &str, x: f64, y: f64) -> json::ViewElement {
    json::ViewElement::Auxiliary(json::AuxiliaryViewElement {
        uid,
        name: name.to_string(),
        x,
        y,
        label_side: String::new(),
    })
}

#[test]
fn a_geometry_edit_implies_no_model_operation_and_is_a_view_only_patch() {
    let mut project = scene();
    let edit = upsert(vec![aux(7, 260.0, 470.0), stock(1, 90.0, 90.0)]);
    assert_eq!(operations(&project, &edit), Ok(Vec::new()));
    assert!(is_view_only_patch(&project, &edit_patch(&edit)));
    apply_edit(&mut project, &edit).expect("the edit applies");
    assert!(view_of(&project).contains(&edit.upsert[0]));
    assert_eq!(agreement_report(&project), "");
}

#[test]
fn each_created_named_element_creates_its_variable() {
    let mut failures = Vec::new();
    for kind in VariableKind::ALL {
        let (elements, want) = match kind {
            VariableKind::Stock => (vec![stock(20, 700.0, 100.0)], "upsert stock s20"),
            VariableKind::Flow => (
                vec![
                    flow(
                        20,
                        (750.0, 400.0),
                        &[(700.0, 400.0, 21), (800.0, 400.0, 22)],
                    ),
                    cloud(21, 20, 700.0, 400.0),
                    cloud(22, 20, 800.0, 400.0),
                ],
                "upsert flow f20",
            ),
            VariableKind::Aux => (vec![aux(20, 700.0, 600.0)], "upsert aux a20"),
            VariableKind::Module => (
                vec![json::ViewElement::Module(json::ModuleViewElement {
                    uid: 20,
                    name: "m20".to_string(),
                    x: 700.0,
                    y: 800.0,
                    label_side: String::new(),
                })],
                "upsert module m20",
            ),
        };
        let mut project = scene();
        let edit = upsert(elements);
        let ops = operations(&project, &edit);
        if ops != Ok(vec![want.to_string()]) {
            failures.push(format!("{kind:?}: {ops:?}, want [{want}]"));
            continue;
        }
        if !is_view_only_patch(&project, &edit_patch(&edit)) {
            if let Err(e) = apply_edit(&mut project, &edit) {
                failures.push(format!("{kind:?}: {e}"));
            }
        } else {
            failures.push(format!("{kind:?}: a create counted as view-only"));
        }
        let report = agreement_report(&project);
        if !report.is_empty() {
            failures.push(format!("{kind:?}: {report}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_relabeled_element_renames_its_variable() {
    let mut project = scene();
    let edit = plan_rename(stock_flow_of(&project), "a7", "Growth Rate");
    assert_eq!(
        operations(&project, &edit),
        Ok(vec!["rename a7 -> Growth Rate".to_string()])
    );
    apply_edit(&mut project, &edit).expect("the rename applies");
    let model = main_model(&project);
    assert!(model.get_variable("growth_rate").is_some());
    assert!(model.get_variable("a7").is_none());
    assert_eq!(agreement_report(&project), "");

    // A line break is stored as its two-character escape, never raw.
    let broken = plan_rename(stock_flow_of(&scene()), "a7", "Growth\nRate");
    assert_eq!(broken.upsert[0].get_name(), Some("Growth\\nRate"));

    // A variable with no element on the view has nothing to relabel.
    assert!(plan_rename(stock_flow_of(&scene()), "no_such_variable", "x").is_empty());
}

#[test]
fn a_delete_removes_what_depends_on_the_selection() {
    struct Delete {
        name: &'static str,
        selection: &'static [i32],
        remove: &'static [i32],
        ops: &'static [&'static str],
    }
    let rows = [
        Delete {
            name: "a stock: its flow's end becomes a cloud",
            selection: &[2],
            remove: &[2],
            ops: &["delete s2"],
        },
        Delete {
            name: "a flow: its clouds and the link into it go too",
            selection: &[4],
            remove: &[4, 5, 6, 8],
            ops: &["delete f4"],
        },
        Delete {
            name: "an aux: its alias and links go too",
            selection: &[7],
            remove: &[7, 8, 9],
            ops: &["delete a7"],
        },
        Delete {
            name: "a lone cloud of a surviving flow: ignored",
            selection: &[5],
            remove: &[],
            ops: &[],
        },
    ];
    let mut failures = Vec::new();
    for row in rows {
        let mut project = scene();
        let edit = plan_delete(stock_flow_of(&project), row.selection);
        if edit.remove != row.remove {
            failures.push(format!(
                "{}: removes {:?}, want {:?}",
                row.name, edit.remove, row.remove
            ));
        }
        let want: Vec<String> = row.ops.iter().map(|s| s.to_string()).collect();
        let ops = operations(&project, &edit);
        if ops != Ok(want.clone()) {
            failures.push(format!("{}: {ops:?}, want {want:?}", row.name));
        }
        if let Err(e) = apply_edit(&mut project, &edit) {
            failures.push(format!("{}: {e}", row.name));
        }
        let report = agreement_report(&project);
        if !report.is_empty() {
            failures.push(format!("{}: {report}", row.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn attaching_and_detaching_a_flow_end_updates_the_stock_lists() {
    let mut failures = Vec::new();
    let rows = [
        (
            "a cloud end attached to a stock joins its inflows",
            ViewEdit {
                upsert: load(vec![flow(
                    4,
                    (250.0, 300.0),
                    &[(100.0, 300.0, 5), (400.0, 300.0, 0), (400.0, 117.5, 2)],
                )]),
                remove: vec![6],
            },
            r#"stock s2 in ["f3", "f4"] out []"#,
        ),
        (
            "a stock end detached into a cloud leaves its inflows",
            upsert(vec![
                flow(3, (211.25, 100.0), &[(122.5, 100.0, 1), (300.0, 100.0, 20)]),
                cloud(20, 3, 300.0, 100.0),
            ]),
            "stock s2 in [] out []",
        ),
    ];
    for (name, edit, want) in rows {
        let mut project = scene();
        let ops = operations(&project, &edit);
        if ops != Ok(vec![want.to_string()]) {
            failures.push(format!("{name}: {ops:?}, want [{want}]"));
        }
        if let Err(e) = apply_edit(&mut project, &edit) {
            failures.push(format!("{name}: {e}"));
        }
        let report = agreement_report(&project);
        if !report.is_empty() {
            failures.push(format!("{name}: {report}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_edit_naming_an_existing_variable_is_refused_and_changes_nothing() {
    let refused = [
        (
            "creating an element under a taken name",
            upsert(vec![named_aux(20, "S1", 700.0, 700.0)]),
        ),
        (
            "relabeling an element onto a taken name",
            upsert(vec![named_aux(7, "s1", 250.0, 450.0)]),
        ),
    ];
    for (name, edit) in refused {
        let mut project = scene();
        let before = project.clone();
        assert!(
            operations(&project, &edit).is_err(),
            "{name}: derived operations"
        );
        assert!(apply_edit(&mut project, &edit).is_err(), "{name}: applied");
        assert!(
            view_of(&project) == view_of(&before),
            "{name}: the view changed"
        );
        assert!(
            main_model(&project).variables == main_model(&before).variables,
            "{name}: the variables changed"
        );
    }

    // The name a rename frees is free for a create in the same edit.
    let mut project = scene();
    let edit = upsert(vec![
        named_aux(7, "Rate", 250.0, 450.0),
        named_aux(20, "a7", 700.0, 700.0),
    ]);
    assert_eq!(
        operations(&project, &edit),
        Ok(vec![
            "rename a7 -> Rate".to_string(),
            "upsert aux a7".to_string()
        ])
    );
    apply_edit(&mut project, &edit).expect("the edit applies");
    assert_eq!(agreement_report(&project), "");
}
