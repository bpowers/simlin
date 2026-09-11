// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incremental layout's flow contract.
//!
//! A flow is rebuilt only when the patch creates it or changes the flow's own
//! attachment: it moves to another stock, an attached stock is deleted, or an
//! attached stock changes kind. Every other flow comes back byte for byte --
//! points, valve, label side and clouds -- even where its stored geometry is
//! not what a fresh layout would draw. A flow the pass builds holds the flow
//! invariants (`diagram::flow_geometry`), and its stock end takes the largest
//! free gap on its face, so it never lands on a preserved sibling. One test per
//! way a patch relates to a flow:
//!
//! - names the flow: an upsert keeps it, a rename keeps all but the name, a
//!   delete removes it with its clouds
//! - changes its attachment -- moves it to another stock, deletes an attached
//!   stock, changes an attached stock's kind: rebuilt
//! - touches only a sibling -- a flow added on its face, a chain flow added on
//!   a face it occupies, its stock's chain removed: preserved, and a created
//!   sibling takes the largest free gap
//! - touches an unrelated element, with a new element (the settle path) and
//!   without one (the early-return path): preserved, an off-pipe valve included
//!
//! Label sides are enumerated in `layout_label_tests.rs`.

use super::*;
use crate::datamodel::{self, view_element::Cloud};
use crate::diagram::flow_geometry::{CORNER_CLEARANCE, flow_invariant_violations};
use crate::patch::{ModelOperation, ModelPatch};

fn stock(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn flow(ident: &str, equation: &str) -> datamodel::Flow {
    datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    }
}

fn aux(ident: &str, equation: &str) -> datamodel::Aux {
    datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    }
}

fn project_of(variables: Vec<datamodel::Variable>) -> datamodel::Project {
    test_project(datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables,
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    })
}

/// stock_a -> chain_flow -> stock_b, plus stock_a -> waste_a -> cloud, whose
/// rate reads the aux leak_rate.
fn chain_and_waste() -> datamodel::Project {
    project_of(vec![
        stock("stock_a", &[], &["chain_flow", "waste_a"]),
        stock("stock_b", &["chain_flow"], &[]),
        datamodel::Variable::Flow(flow("chain_flow", "10")),
        datamodel::Variable::Flow(flow("waste_a", "stock_a * leak_rate")),
        datamodel::Variable::Aux(aux("leak_rate", "0.1")),
    ])
}

/// A flow element by canonical name, with the clouds it owns in uid order.
fn flow_and_clouds(view: &datamodel::StockFlow, ident: &str) -> (view_element::Flow, Vec<Cloud>) {
    let f = find_flow(view, ident).unwrap_or_else(|| panic!("flow {ident} not in view"));
    let mut clouds: Vec<Cloud> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Cloud(c) if c.flow_uid == f.uid => Some(c.clone()),
            _ => None,
        })
        .collect();
    clouds.sort_by_key(|c| c.uid);
    (f, clouds)
}

fn find_flow(view: &datamodel::StockFlow, ident: &str) -> Option<view_element::Flow> {
    view.elements.iter().find_map(|e| match e {
        ViewElement::Flow(f) if canonicalize(&f.name) == ident => Some(f.clone()),
        _ => None,
    })
}

fn stock_named(view: &datamodel::StockFlow, ident: &str) -> view_element::Stock {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name) == ident => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("stock {ident} not in view"))
}

/// The flow invariant violations that concern one flow: its own, and a cloud
/// of its drawn inside a stock.
fn violations_of(view: &datamodel::StockFlow, ident: &str) -> Vec<String> {
    let (f, clouds) = flow_and_clouds(view, ident);
    let own = format!("{}:", f.name);
    let cloud_prefixes: Vec<String> = clouds.iter().map(|c| format!("cloud {} ", c.uid)).collect();
    flow_invariant_violations(&view.elements)
        .into_iter()
        .filter(|v| v.starts_with(&own) || cloud_prefixes.iter().any(|p| v.starts_with(p)))
        .collect()
}

fn set_stock_flows(model: &mut datamodel::Model, ident: &str, inflows: &[&str], outflows: &[&str]) {
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == ident
        {
            s.inflows = inflows.iter().map(|f| f.to_string()).collect();
            s.outflows = outflows.iter().map(|f| f.to_string()).collect();
        }
    }
}

/// Apply `ops` to a copy of `base` edited by `edit` (the post-patch model, as
/// `apply_patch` leaves it) and lay the view out incrementally.
fn incremental(
    base: &datamodel::Project,
    old_view: &datamodel::StockFlow,
    edit: impl FnOnce(&mut datamodel::Model),
    ops: Vec<ModelOperation>,
) -> datamodel::StockFlow {
    let mut patched = base.clone();
    edit(patched.get_model_mut(TEST_MODEL).unwrap());
    let patch = ModelPatch {
        name: TEST_MODEL.to_string(),
        ops,
    };
    incremental_layout(old_view, &patched, TEST_MODEL, &patch, None).expect("incremental layout")
}

fn assert_preserved(
    old: &datamodel::StockFlow,
    new: &datamodel::StockFlow,
    idents: &[&str],
    row: &str,
) {
    for ident in idents {
        assert_eq!(
            flow_and_clouds(new, ident),
            flow_and_clouds(old, ident),
            "{row}: {ident} must come back byte for byte"
        );
    }
}

/// Each stock end of `ident` sits at the center of the gap it occupies on its
/// face, and that gap is the largest one the other flows' ends and the corner
/// clearance leave: stated independently of `face_slots`, from the geometry.
fn assert_in_largest_free_gap(view: &datamodel::StockFlow, ident: &str, row: &str) {
    let config = LayoutConfig::default();
    let (half_w, half_h) = (config.stock_width / 2.0, config.stock_height / 2.0);
    let f = find_flow(view, ident).unwrap();
    let stocks: HashMap<i32, (f64, f64)> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();
    // (along-coordinate, which face) of a point on a stock face, or None.
    let face_of = |p: &FlowPoint, s: (f64, f64)| -> Option<(f64, i8)> {
        let (dx, dy) = (p.x - s.0, p.y - s.1);
        if (dx.abs() - half_w).abs() < 1e-6 && dy.abs() <= half_h {
            Some((p.y, if dx > 0.0 { 0 } else { 1 }))
        } else if (dy.abs() - half_h).abs() < 1e-6 && dx.abs() <= half_w {
            Some((p.x, if dy > 0.0 { 2 } else { 3 }))
        } else {
            None
        }
    };
    let last = f.points.len() - 1;
    for end in [0, last] {
        let p = &f.points[end];
        let Some(&s) = p.attached_to_uid.and_then(|u| stocks.get(&u)) else {
            continue;
        };
        let (at, face) =
            face_of(p, s).unwrap_or_else(|| panic!("{row}: {ident} end off its stock"));
        let (center, half) = if face >= 2 {
            (s.0, half_w)
        } else {
            (s.1, half_h)
        };
        let reach = half - CORNER_CLEARANCE;
        let mut bounds = vec![center - reach, center + reach];
        for e in &view.elements {
            let ViewElement::Flow(g) = e else { continue };
            let g_last = g.points.len() - 1;
            for (i, q) in [(0, &g.points[0]), (g_last, &g.points[g_last])] {
                if (g.uid, i) == (f.uid, end) || q.attached_to_uid != p.attached_to_uid {
                    continue;
                }
                if let Some((along, g_face)) = face_of(q, s)
                    && g_face == face
                {
                    bounds.push(along);
                }
            }
        }
        bounds.sort_by(f64::total_cmp);
        let gaps: Vec<(f64, f64)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();
        let largest = gaps.iter().map(|(lo, hi)| hi - lo).fold(0.0, f64::max);
        let (lo, hi) = gaps
            .iter()
            .copied()
            .find(|&(lo, hi)| lo < at && at < hi)
            .unwrap_or_else(|| panic!("{row}: {ident}'s end at {at} coincides with another end"));
        assert!(
            (at - (lo + hi) / 2.0).abs() < 1e-6 && (hi - lo) >= largest - 1e-6,
            "{row}: {ident}'s end at {at} is not the center of the largest free gap \
             (its gap [{lo}, {hi}], largest {largest})"
        );
    }
}

#[test]
fn a_flow_the_patch_names_keeps_its_geometry() {
    let base = chain_and_waste();
    let old = generate_layout(&base, TEST_MODEL, None).expect("initial layout");

    let new = incremental(
        &base,
        &old,
        |m| {
            for var in &mut m.variables {
                if let datamodel::Variable::Flow(f) = var
                    && f.ident == "waste_a"
                {
                    f.equation = datamodel::Equation::Scalar("stock_a * leak_rate * 2".to_string());
                }
            }
        },
        vec![ModelOperation::UpsertFlow(flow(
            "waste_a",
            "stock_a * leak_rate * 2",
        ))],
    );
    assert_preserved(&old, &new, &["waste_a", "chain_flow"], "upsert");

    let new = incremental(
        &base,
        &old,
        |m| {
            for var in &mut m.variables {
                if let datamodel::Variable::Flow(f) = var
                    && f.ident == "waste_a"
                {
                    f.ident = "spill".to_string();
                }
            }
            set_stock_flows(m, "stock_a", &[], &["chain_flow", "spill"]);
        },
        vec![ModelOperation::RenameVariable {
            from: "waste_a".to_string(),
            to: "spill".to_string(),
        }],
    );
    let (mut renamed, clouds) = flow_and_clouds(&old, "waste_a");
    let (spill, spill_clouds) = flow_and_clouds(&new, "spill");
    renamed.name = spill.name.clone();
    assert_eq!(
        (spill, spill_clouds),
        (renamed, clouds),
        "rename: all but the name"
    );
    assert_preserved(&old, &new, &["chain_flow"], "rename");

    let waste_uid = find_flow(&old, "waste_a").unwrap().uid;
    let new = incremental(
        &base,
        &old,
        |m| {
            m.variables.retain(|v| v.get_ident() != "waste_a");
            set_stock_flows(m, "stock_a", &[], &["chain_flow"]);
        },
        vec![ModelOperation::DeleteVariable {
            ident: "waste_a".to_string(),
        }],
    );
    assert!(
        find_flow(&new, "waste_a").is_none(),
        "delete: the flow is gone"
    );
    assert!(
        !new.elements
            .iter()
            .any(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == waste_uid)),
        "delete: its clouds are gone"
    );
    assert_preserved(&old, &new, &["chain_flow"], "delete");
}

/// One way a patch changes a flow's attachment: the patch, the flow it
/// rebuilds, the stock each end must then attach to (`None`: a cloud of the
/// flow's own), and the flows it must leave alone.
struct Reattachment {
    label: &'static str,
    edit: Box<dyn FnOnce(&mut datamodel::Model)>,
    ops: Vec<ModelOperation>,
    ident: &'static str,
    source: Option<&'static str>,
    sink: Option<&'static str>,
    preserved: &'static [&'static str],
}

#[test]
fn a_flow_whose_attachment_changes_is_rebuilt() {
    let base = chain_and_waste();
    let old = generate_layout(&base, TEST_MODEL, None).expect("initial layout");

    let rows = vec![
        Reattachment {
            label: "moved to another stock",
            edit: Box::new(|m| {
                set_stock_flows(m, "stock_a", &[], &["chain_flow"]);
                set_stock_flows(m, "stock_b", &["chain_flow"], &["waste_a"]);
            }),
            ops: vec![
                ModelOperation::UpdateStockFlows {
                    ident: "stock_a".to_string(),
                    inflows: vec![],
                    outflows: vec!["chain_flow".to_string()],
                },
                ModelOperation::UpdateStockFlows {
                    ident: "stock_b".to_string(),
                    inflows: vec!["chain_flow".to_string()],
                    outflows: vec!["waste_a".to_string()],
                },
            ],
            ident: "waste_a",
            source: Some("stock_b"),
            sink: None,
            preserved: &["chain_flow"],
        },
        Reattachment {
            label: "an attached stock deleted",
            edit: Box::new(|m| m.variables.retain(|v| v.get_ident() != "stock_b")),
            ops: vec![ModelOperation::DeleteVariable {
                ident: "stock_b".to_string(),
            }],
            ident: "chain_flow",
            source: Some("stock_a"),
            sink: None,
            preserved: &["waste_a"],
        },
        Reattachment {
            label: "an attached stock turned into an aux",
            edit: Box::new(|m| {
                m.variables.retain(|v| v.get_ident() != "stock_b");
                m.variables
                    .push(datamodel::Variable::Aux(aux("stock_b", "0")));
            }),
            ops: vec![ModelOperation::UpsertAux(aux("stock_b", "0"))],
            ident: "chain_flow",
            source: Some("stock_a"),
            sink: None,
            preserved: &["waste_a"],
        },
    ];
    for Reattachment {
        label,
        edit,
        ops,
        ident,
        source,
        sink,
        preserved,
    } in rows
    {
        let new = incremental(&base, &old, edit, ops);
        let f = find_flow(&new, ident).unwrap();
        let last = f.points.len() - 1;
        for (end, expected) in [(0, source), (last, sink)] {
            let attached = f.points[end].attached_to_uid;
            match expected {
                Some(s) => assert_eq!(
                    attached,
                    Some(stock_named(&new, s).uid),
                    "{label}: {ident} end {end} attaches to {s}"
                ),
                None => assert!(
                    new.elements.iter().any(
                        |e| matches!(e, ViewElement::Cloud(c) if Some(c.uid) == attached && c.flow_uid == f.uid)
                    ),
                    "{label}: {ident} end {end} attaches to a cloud of its own"
                ),
            }
        }
        assert_eq!(violations_of(&new, ident), Vec::<String>::new(), "{label}");
        assert_preserved(&old, &new, preserved, label);
    }
}

#[test]
fn a_flow_whose_sibling_changes_is_preserved() {
    let base = chain_and_waste();
    let old = generate_layout(&base, TEST_MODEL, None).expect("initial layout");

    let new = incremental(
        &base,
        &old,
        |m| {
            set_stock_flows(m, "stock_a", &[], &["chain_flow", "waste_a", "waste_b"]);
            m.variables
                .push(datamodel::Variable::Flow(flow("waste_b", "1")));
        },
        vec![
            ModelOperation::UpsertFlow(flow("waste_b", "1")),
            ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec![
                    "chain_flow".to_string(),
                    "waste_a".to_string(),
                    "waste_b".to_string(),
                ],
            },
        ],
    );
    let row = "a flow added on its face";
    assert_preserved(&old, &new, &["waste_a", "chain_flow"], row);
    assert_eq!(
        violations_of(&new, "waste_b"),
        Vec::<String>::new(),
        "{row}"
    );
    assert_in_largest_free_gap(&new, "waste_b", row);

    // Classification now puts waste_a on the right face (stock_a has no chain
    // outflow left); the preserved flow stays on the bottom.
    let new = incremental(
        &base,
        &old,
        |m| {
            m.variables
                .retain(|v| v.get_ident() != "chain_flow" && v.get_ident() != "stock_b");
            set_stock_flows(m, "stock_a", &[], &["waste_a"]);
        },
        vec![
            ModelOperation::DeleteVariable {
                ident: "chain_flow".to_string(),
            },
            ModelOperation::DeleteVariable {
                ident: "stock_b".to_string(),
            },
            ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["waste_a".to_string()],
            },
        ],
    );
    assert_preserved(&old, &new, &["waste_a"], "its stock's chain removed");

    let waste_only = project_of(vec![
        stock("stock_a", &[], &["waste_flow"]),
        datamodel::Variable::Flow(flow("waste_flow", "5")),
    ]);
    let old = generate_layout(&waste_only, TEST_MODEL, None).expect("initial layout");
    let new = incremental(
        &waste_only,
        &old,
        |m| {
            set_stock_flows(m, "stock_a", &[], &["waste_flow", "chain_flow"]);
            m.variables.push(stock("stock_b", &["chain_flow"], &[]));
            m.variables
                .push(datamodel::Variable::Flow(flow("chain_flow", "10")));
        },
        vec![
            ModelOperation::UpsertStock(match stock("stock_b", &["chain_flow"], &[]) {
                datamodel::Variable::Stock(s) => s,
                _ => unreachable!(),
            }),
            ModelOperation::UpsertFlow(flow("chain_flow", "10")),
            ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["waste_flow".to_string(), "chain_flow".to_string()],
            },
        ],
    );
    let row = "a chain flow added on a face it occupies";
    assert_preserved(&old, &new, &["waste_flow"], row);
    assert_eq!(
        violations_of(&new, "chain_flow"),
        Vec::<String>::new(),
        "{row}"
    );
    assert_in_largest_free_gap(&new, "chain_flow", row);
}

/// The old view's waste_a has its valve moved off its pipe: stored geometry
/// that breaks the invariants (a hand edit, or a view saved before they held)
/// is exactly what no pass may "repair" on a flow the patch did not touch.
#[test]
fn a_flow_the_patch_does_not_touch_is_preserved() {
    let base = chain_and_waste();
    let mut old = generate_layout(&base, TEST_MODEL, None).expect("initial layout");
    for e in &mut old.elements {
        if let ViewElement::Flow(f) = e
            && canonicalize(&f.name) == "waste_a"
        {
            f.x += 30.0;
        }
    }
    assert!(
        !violations_of(&old, "waste_a").is_empty(),
        "fixture: waste_a's valve is off its pipe"
    );

    let new = incremental(
        &base,
        &old,
        |m| {
            m.variables
                .push(datamodel::Variable::Aux(aux("unrelated", "1")))
        },
        vec![ModelOperation::UpsertAux(aux("unrelated", "1"))],
    );
    assert!(
        new.elements
            .iter()
            .any(|e| matches!(e, ViewElement::Aux(a) if canonicalize(&a.name) == "unrelated")),
        "fixture: the patch creates an element, so the settle path runs"
    );
    assert_preserved(
        &old,
        &new,
        &["waste_a", "chain_flow"],
        "a new unrelated element",
    );

    let new = incremental(
        &base,
        &old,
        |m| {
            for var in &mut m.variables {
                if let datamodel::Variable::Aux(a) = var
                    && a.ident == "leak_rate"
                {
                    a.equation = datamodel::Equation::Scalar("0.2".to_string());
                }
            }
        },
        vec![ModelOperation::UpsertAux(aux("leak_rate", "0.2"))],
    );
    assert_preserved(
        &old,
        &new,
        &["waste_a", "chain_flow"],
        "an unrelated edit, no new element",
    );
}
