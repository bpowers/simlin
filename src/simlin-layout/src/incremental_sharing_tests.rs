// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a sync leaves in place it shares with the diagram it was given.

use simlin_base::datamodel::{Equation, Project, SharedVec, Variable, View, ViewElement};
use simlin_base::project_patch::{ModelOperation, ModelPatch, ProjectPatch};
use simlin_base::shared_vec::changed_by;
use simlin_edit::apply_patch;

fn world3() -> Project {
    simlin_formats::compat::open_vensim(include_str!("../../../test/metasd/WRLD3-03/wrld3-03.mdl"))
        .expect("world3 opens")
}

fn tweaked(equation: &Equation) -> Equation {
    let mut equation = equation.clone();
    match &mut equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => text.push_str(" + 0"),
        Equation::Arrayed(_, elements, _, _) => {
            if let Some((_, text, _, _)) = elements.first_mut() {
                text.push_str(" + 0");
            }
        }
    }
    equation
}

fn elements(project: &Project) -> &SharedVec<ViewElement> {
    match &project.models[0].views[0] {
        View::StockFlow(sf) => &sf.elements,
    }
}

#[test]
fn a_layout_sync_shares_every_element_it_leaves_in_place() {
    let original = world3();
    let mut copy = original.clone();
    let aux = original.models[0]
        .variables
        .iter()
        .rev()
        .find_map(|v| match v {
            Variable::Aux(a) if matches!(a.equation, Equation::Scalar(_)) => Some(a.clone()),
            _ => None,
        })
        .expect("world3 has a scalar aux");
    let mut edited = aux;
    edited.equation = tweaked(&edited.equation);
    let patch = ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops: vec![ModelOperation::UpsertAux(edited)],
        }],
    };
    apply_patch(&mut copy, patch.clone()).unwrap();
    let View::StockFlow(old) = &copy.models[0].views[0];
    let old = old.clone();
    // The layout lays every element out afresh, as a host's diagram sync
    // does after each edit.
    let synced = super::incremental_layout(&old, &copy, "main", &patch.models[0], None)
        .expect("the layout syncs");
    copy.models[0].views[0] = View::StockFlow(synced);

    let moved = changed_by(
        elements(&original),
        elements(&copy),
        ViewElement::get_uid,
        |a, b| a == b,
        "a sync after an equation edit",
    );
    assert!(moved.is_empty(), "an equation edit moves nothing");
}
