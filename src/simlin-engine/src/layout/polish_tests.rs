// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel::view_element::{self, LabelSide, LinkShape};

fn aux(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn stock(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
    ViewElement::Stock(view_element::Stock {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn link(uid: i32, from_uid: i32, to_uid: i32) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape: LinkShape::Straight,
        polarity: None,
    })
}

fn view_of(elements: Vec<ViewElement>) -> datamodel::StockFlow {
    datamodel::StockFlow {
        name: None,
        elements,
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    }
}

fn position(elements: &[ViewElement], uid: i32) -> (f64, f64) {
    elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Aux(a) if a.uid == uid => Some((a.x, a.y)),
            ViewElement::Stock(s) if s.uid == uid => Some((s.x, s.y)),
            _ => None,
        })
        .expect("node drawn")
}

#[test]
fn a_parameter_on_the_wrong_side_steps_off_the_crossing() {
    // Two stocks side by side, each read by a parameter drawn below the OTHER
    // stock, so the two links cross in an X. Moving either parameter around
    // its stock uncrosses them; the stocks never move.
    let elements = vec![
        stock(1, "left stock", 0.0, 0.0),
        stock(2, "right stock", 200.0, 0.0),
        aux(3, "left parameter", 200.0, 120.0),
        aux(4, "right parameter", 0.0, 120.0),
        link(10, 3, 1),
        link(11, 4, 2),
    ];
    let before = count_view_crossings(&view_of(elements.clone()));
    assert_eq!(before, 1, "fixture: the two links cross");

    let mut polished = elements.clone();
    polish_crossings(&mut polished);

    assert_eq!(count_view_crossings(&view_of(polished.clone())), 0);
    assert_eq!(position(&polished, 1), position(&elements, 1));
    assert_eq!(position(&polished, 2), position(&elements, 2));
}

#[test]
fn a_crossing_free_diagram_is_left_alone() {
    let elements = vec![
        stock(1, "left stock", 0.0, 0.0),
        stock(2, "right stock", 200.0, 0.0),
        aux(3, "left parameter", 0.0, 120.0),
        aux(4, "right parameter", 200.0, 120.0),
        link(10, 3, 1),
        link(11, 4, 2),
    ];
    let mut polished = elements.clone();
    polish_crossings(&mut polished);
    assert!(polished == elements, "nothing crosses, so nothing moves");
}

#[test]
fn a_parameter_never_steps_onto_another_shape() {
    // A parameter reads down into its stock across a long link between two
    // far auxes. Every spot on its ring above that link would uncross it, and
    // every one of them is taken by a module; the polish must keep the
    // crossing rather than drop the parameter onto a module.
    let mut elements = vec![
        stock(1, "consumer", 0.0, 0.0),
        aux(2, "crossed parameter", 0.0, 100.0),
        aux(3, "a", -400.0, 50.0),
        aux(4, "b", 400.0, 50.0),
        link(10, 2, 1),
        link(11, 3, 4),
    ];
    for k in 0..16 {
        let angle = k as f64 * 2.0 * PI / 16.0;
        let (x, y) = (100.0 * angle.cos(), 100.0 * angle.sin());
        if y < 50.0 {
            elements.push(ViewElement::Module(view_element::Module {
                name: format!("module {k}"),
                uid: 100 + k,
                x,
                y,
                label_side: LabelSide::Bottom,
            }));
        }
    }
    let before = elements.clone();
    polish_crossings(&mut elements);
    assert_eq!(
        position(&elements, 2),
        position(&before, 2),
        "the parameter stays put"
    );
}
