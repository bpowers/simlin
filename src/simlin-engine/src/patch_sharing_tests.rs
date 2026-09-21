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
use crate::shared_vec::Identical;

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
    changed_by(before, after, key, |a, b| a == b)
}

/// `changed`, with `same` deciding what counts as left as it was.
fn changed_by<T: Clone, K: Eq + Hash + Clone + std::fmt::Debug>(
    before: &SharedVec<T>,
    after: &SharedVec<T>,
    key: impl Fn(&T) -> K,
    same: impl Fn(&T, &T) -> bool,
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
            Some((before_address, before_value)) if same(before_value, e) => assert_eq!(
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

/// The keys of the view elements that differ from the original's, after
/// asserting that every element identical to the original's shares its
/// allocation.
fn view_changed(original: &datamodel::Project, edited: &datamodel::Project) -> Vec<i32> {
    changed_by(
        elements(original),
        elements(edited),
        ViewElement::get_uid,
        |a, b| a.identical(b),
    )
}

#[test]
fn replacing_a_whole_view_shares_every_element_it_keeps() {
    let original = world3();
    let mut copy = original.clone();
    // A host's replacement view arrives with every element freshly
    // allocated, one of them moved.
    let View::StockFlow(sf) = &original.models[0].views[0];
    let mut fresh = sf.elements.to_vec();
    let moved = fresh
        .iter_mut()
        .find_map(|e| match e {
            ViewElement::Stock(s) => {
                s.y += 10.0;
                Some(s.uid)
            }
            _ => None,
        })
        .expect("world3 draws a stock");
    let replacement = datamodel::StockFlow {
        elements: fresh.into(),
        ..sf.clone()
    };
    apply_patch(
        &mut copy,
        model_patch(vec![ModelOperation::UpsertView {
            index: 0,
            view: View::StockFlow(replacement),
        }]),
    )
    .unwrap();

    assert_eq!(view_changed(&original, &copy), [moved]);
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
    if let Equation::Scalar(eq) = &mut edited.equation {
        eq.push_str(" + 0");
    }
    let patch = model_patch(vec![ModelOperation::UpsertAux(edited)]);
    apply_patch(&mut copy, patch.clone()).unwrap();
    let View::StockFlow(old) = &copy.models[0].views[0];
    let old = old.clone();
    // The layout lays every element out afresh, as a host's diagram sync
    // does after each edit.
    let synced = crate::layout::incremental_layout(&old, &copy, "main", &patch.models[0], None)
        .expect("the layout syncs");
    copy.models[0].views[0] = View::StockFlow(synced);

    assert!(
        view_changed(&original, &copy).is_empty(),
        "an equation edit moves nothing"
    );
}

fn aux(ident: &str, equation: &str, module_input: bool) -> Variable {
    Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            can_be_module_input: module_input,
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
    })
}

fn module(ident: &str, references: &[(&str, &str)]) -> Variable {
    Variable::Module(datamodel::Module {
        ident: ident.to_string(),
        model_name: "submodel".to_string(),
        documentation: String::new(),
        units: None,
        references: references
            .iter()
            .map(|(src, dst)| datamodel::ModuleReference {
                src: src.to_string(),
                dst: format!("{ident}\u{00B7}{dst}"),
            })
            .collect(),
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

/// A main model with three instances of one submodel: `wired` feeds its
/// first port from `local_input`, `other` feeds its second from
/// `other_input`, and `bare` is wired to nothing.
fn modules_project() -> datamodel::Project {
    let mut project = crate::test_common::TestProject::new("test").build_datamodel();
    let main = &mut project.models[0];
    main.variables.extend([
        aux("local_input", "10", false),
        aux("other_input", "20", false),
        module("wired", &[("local_input", "input_a")]),
        module("other", &[("other_input", "input_b")]),
        module("bare", &[]),
    ]);
    project.models.push(datamodel::Model {
        name: "submodel".to_string(),
        sim_specs: None,
        variables: vec![
            aux("input_a", "0", true),
            aux("input_b", "0", true),
            aux("output", "input_a + input_b", false),
        ]
        .into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project
}

fn rename_in(model: &str, from: &str, to: &str) -> ProjectPatch {
    ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: model.to_string(),
            ops: vec![ModelOperation::RenameVariable {
                from: from.to_string(),
                to: to.to_string(),
            }],
        }],
    }
}

#[test]
fn a_rename_copies_only_the_modules_that_name_it() {
    let original = modules_project();
    let mut copy = original.clone();
    apply_patch(&mut copy, rename_in("main", "local_input", "renamed_input")).unwrap();

    let mut variables = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    );
    variables.sort();
    assert_eq!(variables, ["renamed_input", "wired"]);
}

#[test]
fn renaming_a_port_copies_only_the_instances_wired_into_it() {
    let original = modules_project();
    let mut copy = original.clone();
    apply_patch(&mut copy, rename_in("submodel", "input_a", "renamed_port")).unwrap();

    assert_eq!(
        changed(
            &original.models[0].variables,
            &copy.models[0].variables,
            ident
        ),
        ["wired"]
    );
    let mut submodel = changed(
        &original.models[1].variables,
        &copy.models[1].variables,
        ident,
    );
    submodel.sort();
    assert_eq!(submodel, ["output", "renamed_port"]);
}

#[test]
fn renaming_a_flow_copies_only_the_stocks_it_touches() {
    let original = world3();
    let mut copy = original.clone();
    let vars = &original.models[0].variables;
    let flow = vars
        .iter()
        .find_map(|v| match v {
            Variable::Flow(f) => Some(f.ident.clone()),
            _ => None,
        })
        .expect("world3 has a flow");
    let canonical_flow = canonicalize(&flow).into_owned();
    // The stocks that name the flow, and any whose lists the rename's sort
    // puts in order; no other stock changes.
    let mut expected: Vec<String> = vars
        .iter()
        .filter_map(|v| match v {
            Variable::Stock(s)
                if s.inflows
                    .iter()
                    .chain(&s.outflows)
                    .any(|f| canonicalize(f) == canonical_flow)
                    || !s.inflows.is_sorted()
                    || !s.outflows.is_sorted() =>
            {
                Some(canonicalize(&s.ident).into_owned())
            }
            _ => None,
        })
        .collect();
    apply_patch(&mut copy, rename_in("main", &flow, "renamed flow")).unwrap();

    let stocks: Vec<String> = changed(
        &original.models[0].variables,
        &copy.models[0].variables,
        ident,
    )
    .into_iter()
    .filter(|k| matches!(copy.models[0].get_variable(k), Some(Variable::Stock(_))))
    .collect();
    let mut stocks = stocks;
    stocks.sort();
    expected.sort();
    assert!(!expected.is_empty());
    assert_eq!(stocks, expected);
}
