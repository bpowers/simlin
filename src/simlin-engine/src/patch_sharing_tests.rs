// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A patch applied to a copy of a project copies only what it changes: every
//! variable and view element it leaves as it was stays shared with the
//! original, for each kind of edit a host lands. `SharedVec` makes a mutation
//! name what it touches; these rows pin that each edit path touches no more.

use std::collections::HashMap;
use std::hash::Hash;

use crate::canonicalize;
use crate::datamodel::{self, Equation, SharedVec, Variable, View, ViewElement};
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};

fn world3() -> datamodel::Project {
    crate::compat::open_vensim(include_str!("../../../test/metasd/WRLD3-03/wrld3-03.mdl"))
        .expect("world3 opens")
}

fn model_patch(ops: Vec<ModelOperation>) -> ProjectPatch {
    ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops,
        }],
    }
}

fn elements(project: &datamodel::Project) -> &SharedVec<ViewElement> {
    match &project.models[0].views[0] {
        View::StockFlow(sf) => &sf.elements,
    }
}

/// The keys of the elements an edit changed, after asserting that every
/// element present on both sides with the same value is the same allocation:
/// nothing was copied that the edit left as it was.
fn changed<T: Clone + PartialEq, K: Eq + Hash + Clone + std::fmt::Debug>(
    before: &SharedVec<T>,
    after: &SharedVec<T>,
    key: impl Fn(&T) -> K,
) -> Vec<K> {
    let before_at: HashMap<K, (usize, &T)> = before
        .iter()
        .zip(before.addresses())
        .map(|(e, a)| (key(e), (a, e)))
        .collect();
    let mut changed = Vec::new();
    for (e, address) in after.iter().zip(after.addresses()) {
        let k = key(e);
        match before_at.get(&k) {
            Some((before_address, before_value)) if *before_value == e => assert_eq!(
                *before_address, address,
                "{k:?} is unchanged but was copied"
            ),
            _ => changed.push(k),
        }
    }
    changed
}

fn ident(v: &Variable) -> String {
    canonicalize(v.get_ident()).into_owned()
}

#[test]
fn an_equation_edit_copies_only_the_variable_it_writes() {
    let original = world3();
    let mut copy = original.clone();
    // The last aux with a scalar equation, so an edit that found its variable
    // by walking the list would reach nearly every other one first.
    let aux = original.models[0]
        .variables
        .iter()
        .rev()
        .find_map(|v| match v {
            Variable::Aux(a) if matches!(a.equation, Equation::Scalar(_)) => Some(a.clone()),
            _ => None,
        })
        .expect("world3 has a scalar aux");
    let mut edited = aux.clone();
    if let Equation::Scalar(eq) = &mut edited.equation {
        eq.push_str(" + 0");
    }
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::UpsertAux(edited)]),
    )
    .unwrap();

    let variables = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    );
    assert_eq!(variables, [canonicalize(&aux.ident).into_owned()]);
    assert!(changed(elements(&original), elements(&copy), ViewElement::get_uid).is_empty());
}

#[test]
fn a_moved_element_copies_only_that_element() {
    let original = world3();
    let mut copy = original.clone();
    let mut moved = elements(&original)
        .iter()
        .find(|e| matches!(e, ViewElement::Aux(_)))
        .cloned()
        .expect("world3 draws an aux");
    if let ViewElement::Aux(a) = &mut moved {
        a.x += 1.0;
    }
    let uid = moved.get_uid();
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::EditView {
            index: 0,
            upsert: vec![moved],
            remove: vec![],
        }]),
    )
    .unwrap();

    assert_eq!(
        changed(elements(&original), elements(&copy), ViewElement::get_uid),
        [uid]
    );
    assert!(
        changed(
            &original.models[0].variables,
            &copy.models[0].variables,
            ident
        )
        .is_empty()
    );
}

#[test]
fn a_rename_copies_only_the_variable_and_its_readers() {
    let original = world3();
    let mut copy = original.clone();
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::RenameVariable {
            from: "population".to_string(),
            to: "world population".to_string(),
        }]),
    )
    .unwrap();

    let variables = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    );
    // The renamed variable and each variable whose equation reads it, and no
    // other: `changed` has asserted everything else is still shared.
    let readers = original.models[0]
        .variables
        .iter()
        .filter(|v| {
            v.get_equation()
                .is_some_and(|e| format!("{e:?}").to_lowercase().contains("population"))
        })
        .count();
    assert!(variables.len() > 1, "the rename rewrote its readers");
    assert!(variables.len() <= readers + 1);
    assert!(variables.len() < original.models[0].variables.len() / 2);
}

#[test]
fn deleting_a_flow_copies_only_the_stocks_it_touched() {
    let original = world3();
    let mut copy = original.clone();
    let flow = original.models[0]
        .variables
        .iter()
        .find_map(|v| match v {
            Variable::Flow(f) => Some(canonicalize(&f.ident).into_owned()),
            _ => None,
        })
        .expect("world3 has a flow");
    let touched: Vec<String> = original.models[0]
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Stock(s)
                if s.inflows
                    .iter()
                    .chain(&s.outflows)
                    .any(|f| canonicalize(f) == flow) =>
            {
                Some(canonicalize(&s.ident).into_owned())
            }
            _ => None,
        })
        .collect();
    assert!(!touched.is_empty(), "the flow fills or drains a stock");
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::DeleteVariable {
            ident: flow.clone(),
        }]),
    )
    .unwrap();

    let mut variables = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    );
    variables.sort();
    let mut expected = touched;
    expected.sort();
    assert_eq!(variables, expected);
    assert!(copy.models[0].get_variable(&flow).is_none());
}

#[test]
fn connecting_a_flow_copies_only_the_stock_it_connects() {
    let original = world3();
    let mut copy = original.clone();
    let (stock, flow) = {
        let vars = &original.models[0].variables;
        let stock = vars
            .iter()
            .find_map(|v| match v {
                Variable::Stock(s) => Some(s.clone()),
                _ => None,
            })
            .expect("world3 has a stock");
        let flow = vars
            .iter()
            .find_map(|v| match v {
                Variable::Flow(f)
                    if !stock
                        .inflows
                        .iter()
                        .chain(&stock.outflows)
                        .any(|x| x == &f.ident) =>
                {
                    Some(f.ident.clone())
                }
                _ => None,
            })
            .expect("a flow the stock doesn't have");
        (stock, flow)
    };
    let mut outflows = stock.outflows.clone();
    outflows.push(flow);
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::UpdateStockFlows {
            ident: stock.ident.clone(),
            inflows: stock.inflows.clone(),
            outflows,
        }]),
    )
    .unwrap();

    let variables = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    );
    assert_eq!(variables, [canonicalize(&stock.ident).into_owned()]);
}
