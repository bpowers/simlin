// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incremental layout's flow contract.
//!
//! A flow is rebuilt only when the patch creates it or changes the flow's own
//! attachment: it moves to another stock, it is dropped from a stock's list or
//! listed on a stock at its cloud end, an attached stock is deleted, or an
//! attached stock changes kind. Every other flow comes back byte for byte --
//! points, valve, label side and clouds -- even where its stored geometry is
//! not what a fresh layout would draw. A flow the pass builds holds the flow
//! invariants (`diagram::flow_geometry`), and its stock end keeps
//! `PIPE_SPACING` from the ends already on its face where the face has room
//! (the design plan's routing preference). One test per way a patch relates to
//! a flow:
//!
//! - names the flow: an upsert keeps it, a rename keeps all but the name, a
//!   delete removes it with its clouds
//! - changes its attachment -- moves it to another stock, drops it from a
//!   stock's list (that end becomes a cloud), lists it on a stock at its cloud
//!   end (the cloud end becomes the stock), deletes an attached stock, changes
//!   an attached stock's kind: rebuilt
//! - touches only a sibling -- a flow added on its face, a chain flow added on
//!   a face it occupies, its stock's chain removed: preserved, and a created
//!   sibling keeps the spacing
//! - creates flows on an imported view (`SIR.xmile`, `mark2.mdl`, and a
//!   reverse flow between offset stocks imported from XMILE): they keep the
//!   spacing from the ends already there, their clouds stay off other clouds
//!   and pipes, and nothing else moves
//! - touches an unrelated element, with a new element (the settle path) and
//!   without one (the early-return path): preserved, a diagonal pipe, an
//!   off-face endpoint and an off-pipe valve included
//!
//! Label sides are enumerated in `layout_label_tests.rs`.

use super::*;
use crate::datamodel::{self, view_element::Cloud};
use crate::diagram::constants::CLOUD_RADIUS;
use crate::diagram::flow_geometry::{
    CORNER_CLEARANCE, PIPE_SPACING, VALVE_CLAMP_MARGIN, flow_invariant_violations,
};
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

/// Each stock end of `ident` keeps the design plan's routing preference on its
/// face, stated from the geometry independently of `face_slots`: at least
/// `PIPE_SPACING` from every other end on that face when the face's clearance
/// span has such a position, and otherwise as far from them as the span
/// allows (sampled at a quarter pixel).
fn assert_slot_keeps_spacing(view: &datamodel::StockFlow, ident: &str, row: &str) {
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
        let mut others: Vec<f64> = Vec::new();
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
                    others.push(along);
                }
            }
        }
        let least = |v: f64| {
            others
                .iter()
                .map(|o| (o - v).abs())
                .fold(f64::INFINITY, f64::min)
        };
        let samples = (8.0 * reach) as usize;
        let best = (0..=samples)
            .map(|i| center - reach + 2.0 * reach * i as f64 / samples as f64)
            .map(least)
            .fold(0.0, f64::max);
        let got = least(at);
        if best >= PIPE_SPACING {
            assert!(
                got >= PIPE_SPACING - 1e-6,
                "{row}: {ident}'s end at {at} is {got} from another end on its face, \
                 under PIPE_SPACING though the face has room"
            );
        } else {
            assert!(
                got >= best - 0.25,
                "{row}: {ident}'s end at {at} is {got} from another end on its face, \
                 though the face offers {best}"
            );
        }
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
            label: "a stock drops the flow from its list (that end becomes a cloud)",
            edit: Box::new(|m| set_stock_flows(m, "stock_a", &[], &["chain_flow"])),
            ops: vec![ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["chain_flow".to_string()],
            }],
            ident: "waste_a",
            source: None,
            sink: None,
            preserved: &["chain_flow"],
        },
        Reattachment {
            label: "a stock lists the flow at its cloud end (the cloud end becomes the stock)",
            edit: Box::new(|m| set_stock_flows(m, "stock_b", &["chain_flow", "waste_a"], &[])),
            ops: vec![ModelOperation::UpdateStockFlows {
                ident: "stock_b".to_string(),
                inflows: vec!["chain_flow".to_string(), "waste_a".to_string()],
                outflows: vec![],
            }],
            ident: "waste_a",
            source: Some("stock_a"),
            sink: Some("stock_b"),
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
    assert_slot_keeps_spacing(&new, "waste_b", row);

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
    assert_slot_keeps_spacing(&new, "chain_flow", row);
}

/// Created flows on production views, through the importers: the flows a
/// patch creates hold the invariants and land clear of the flows already
/// there, and every pre-existing flow comes back byte for byte. Rows:
///
/// - `SIR.xmile`: `relapse` from `infectious` back to `susceptible`, whose
///   facing faces already carry `succumbing`'s ends. The pipe runs straight
///   on one line a pipe spacing off those ends (the joint slot, pinned on its
///   own by `face_slots::tests::place_created_flow_ends_rows`).
/// - `mark2.mdl`: two cloud outflows added at once to `risk taking behavior`,
///   whose face a pre-existing flow occupies. Their ends keep the spacing,
///   and their clouds overlap no other cloud and lie across no other pipe.
#[test]
fn a_flow_created_on_an_imported_view_lands_clear_of_its_neighbours() {
    fn pre_existing_flows(view: &datamodel::StockFlow) -> Vec<(view_element::Flow, Vec<Cloud>)> {
        view.elements
            .iter()
            .filter_map(|e| match e {
                ViewElement::Flow(f) => Some(flow_and_clouds(view, &canonicalize(&f.name))),
                _ => None,
            })
            .collect()
    }
    fn stock_lists(project: &datamodel::Project, ident: &str) -> (Vec<String>, Vec<String>) {
        project.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if canonicalize(&s.ident) == ident => {
                    Some((s.inflows.clone(), s.outflows.clone()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("stock {ident}"))
    }
    fn incremental_on_import(
        project: &datamodel::Project,
        new_flows: &[&str],
        lists: &[(&str, Vec<String>, Vec<String>)],
    ) -> (datamodel::StockFlow, datamodel::StockFlow) {
        let model_name = project.models[0].name.clone();
        let datamodel::View::StockFlow(old) = &project.models[0].views[0];
        let mut patched = project.clone();
        let model = &mut patched.models[0];
        let mut ops = Vec::new();
        for name in new_flows {
            model
                .variables
                .push(datamodel::Variable::Flow(flow(name, "1")));
            ops.push(ModelOperation::UpsertFlow(flow(name, "1")));
        }
        for (ident, inflows, outflows) in lists {
            for var in &mut model.variables {
                if let datamodel::Variable::Stock(s) = var
                    && canonicalize(&s.ident) == *ident
                {
                    s.inflows = inflows.clone();
                    s.outflows = outflows.clone();
                }
            }
            ops.push(ModelOperation::UpdateStockFlows {
                ident: ident.to_string(),
                inflows: inflows.clone(),
                outflows: outflows.clone(),
            });
        }
        let patch = ModelPatch {
            name: model_name.clone(),
            ops,
        };
        let new = incremental_layout(old, &patched, &model_name, &patch, None)
            .expect("incremental layout");
        (old.clone(), new)
    }
    fn assert_pre_existing_preserved(
        old: &datamodel::StockFlow,
        new: &datamodel::StockFlow,
        row: &str,
    ) {
        for (f, clouds) in pre_existing_flows(old) {
            assert_eq!(
                flow_and_clouds(new, &canonicalize(&f.name)),
                (f.clone(), clouds),
                "{row}: {} must come back byte for byte",
                f.name
            );
        }
    }

    const SIR: &str = include_str!("../../../../test/test-models/samples/SIR/SIR.xmile");
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(SIR.as_bytes()))
        .expect("SIR imports");
    let (infectious_in, mut infectious_out) = stock_lists(&project, "infectious");
    let (mut susceptible_in, susceptible_out) = stock_lists(&project, "susceptible");
    infectious_out.push("relapse".to_string());
    susceptible_in.push("relapse".to_string());
    let (old, new) = incremental_on_import(
        &project,
        &["relapse"],
        &[
            ("infectious", infectious_in, infectious_out),
            ("susceptible", susceptible_in, susceptible_out),
        ],
    );
    let row = "SIR relapse";
    assert_pre_existing_preserved(&old, &new, row);
    assert_eq!(
        violations_of(&new, "relapse"),
        Vec::<String>::new(),
        "{row}"
    );
    let relapse = find_flow(&new, "relapse").unwrap();
    assert_eq!(relapse.points.len(), 2, "{row}: a straight pipe");
    assert_slot_keeps_spacing(&new, "relapse", row);

    const MARK2: &str = include_str!("../../../../test/bobby/vdf/econ/mark2.mdl");
    let project = crate::compat::open_vensim(MARK2).expect("mark2 imports");
    let (inflows, mut outflows) = stock_lists(&project, "risk_taking_behavior");
    outflows.push("probe_a".to_string());
    outflows.push("probe_b".to_string());
    let (old, new) = incremental_on_import(
        &project,
        &["probe_a", "probe_b"],
        &[("risk_taking_behavior", inflows, outflows)],
    );
    let row = "mark2 two outflows";
    assert_pre_existing_preserved(&old, &new, row);
    let clouds: Vec<(i32, f64, f64)> = new
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Cloud(c) => Some((c.uid, c.x, c.y)),
            _ => None,
        })
        .collect();
    for probe in ["probe_a", "probe_b"] {
        assert_eq!(
            violations_of(&new, probe),
            Vec::<String>::new(),
            "{row}: {probe}"
        );
        assert_slot_keeps_spacing(&new, probe, row);
        let (probe_flow, probe_clouds) = flow_and_clouds(&new, probe);
        for c in &probe_clouds {
            for &(other, x, y) in &clouds {
                if other == c.uid {
                    continue;
                }
                assert!(
                    (c.x - x).hypot(c.y - y) >= 2.0 * CLOUD_RADIUS - 1e-6,
                    "{row}: {probe}'s cloud at ({}, {}) overlaps cloud {other} at ({x}, {y})",
                    c.x,
                    c.y
                );
            }
            for e in &new.elements {
                let ViewElement::Flow(g) = e else { continue };
                if g.uid == probe_flow.uid {
                    continue;
                }
                for w in g.points.windows(2) {
                    let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 == 0.0 {
                        0.0
                    } else {
                        (((c.x - w[0].x) * dx + (c.y - w[0].y) * dy) / len2).clamp(0.0, 1.0)
                    };
                    let d = (c.x - (w[0].x + t * dx)).hypot(c.y - (w[0].y + t * dy));
                    assert!(
                        d >= CLOUD_RADIUS - 1e-6,
                        "{row}: {probe}'s cloud at ({}, {}) lies across {}'s pipe ({d} away)",
                        c.x,
                        c.y,
                        g.name
                    );
                }
            }
        }
    }
}

/// A reverse flow added between two stocks offset along their facing faces,
/// through the XMILE importer and incremental layout: the existing flow `f1`
/// comes back byte for byte, the created flow holds the flow invariants, and
/// each of its stock ends keeps `PIPE_SPACING` from `f1`'s end on the same
/// face, and its valve sits `VALVE_CLAMP_MARGIN` from the ends of its segment
/// and from `f1`'s pipe. Rows: stock B's offset below A across the range where
/// the two faces' clearance spans overlap (0 to 29), with `f1`'s line at the
/// ends and the middle of that overlap; offsets where the overlap is a sliver
/// within a spacing of `f1`'s ends, so no joint line keeps the spacing and each
/// end takes its own slot (B 28 below with `f1` at 114; 26 and 27 at 113; 27 at
/// 114; 24 and 25 at 112); and Z routes crossing `f1` whose valve the finishing
/// pass would otherwise leave on the riser near a bend or on `f1` (B 11 below
/// at 105, 15 at 110, 19 at 114).
#[test]
fn a_flow_created_between_offset_stocks_keeps_its_spacing() {
    const XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>joint</name><vendor>simlin</vendor><product version="1.0">simlin</product></header>
  <sim_specs><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="A"><eqn>10</eqn><outflow>f1</outflow></stock>
      <stock name="B"><eqn>10</eqn><inflow>f1</inflow></stock>
      <flow name="f1"><eqn>1</eqn></flow>
    </variables>
    <views>
      <view>
        <stock x="100" y="100" name="A"/>
        <stock x="300" y="STOCK_B_Y" name="B"/>
        <flow x="200" y="F1_Y" name="f1">
          <pts>
            <pt x="122.5" y="F1_Y"/>
            <pt x="277.5" y="F1_Y"/>
          </pts>
        </flow>
      </view>
    </views>
  </model>
</xmile>"#;
    let mut rows: Vec<(i32, i32)> = vec![
        (28, 114),
        (26, 113),
        (27, 113),
        (27, 114),
        (24, 112),
        (25, 112),
        (11, 105),
        (15, 110),
        (19, 114),
    ];
    for offset in [0, 1, 2, 10, 14, 20, 29] {
        let lo = (86 + offset).max(86);
        let hi = (114 + offset).min(114);
        rows.extend([(offset, lo), (offset, (lo + hi) / 2), (offset, hi)]);
    }
    for (offset, line) in rows {
        let row = format!("B {offset} below A, f1 at {line}");
        let text = XML
            .replace("STOCK_B_Y", &(100 + offset).to_string())
            .replace("F1_Y", &line.to_string());
        let project = crate::compat::open_xmile(&mut std::io::BufReader::new(text.as_bytes()))
            .expect("the view imports");
        let model_name = project.models[0].name.clone();
        let datamodel::View::StockFlow(old) = &project.models[0].views[0];
        let mut patched = project.clone();
        let model = &mut patched.models[0];
        model
            .variables
            .push(datamodel::Variable::Flow(flow("back", "1")));
        for var in &mut model.variables {
            if let datamodel::Variable::Stock(s) = var {
                let (inflows, outflows): (&[&str], &[&str]) = match canonicalize(&s.ident).as_ref()
                {
                    "a" => (&["back"], &["f1"]),
                    _ => (&["f1"], &["back"]),
                };
                s.inflows = inflows.iter().map(|f| f.to_string()).collect();
                s.outflows = outflows.iter().map(|f| f.to_string()).collect();
            }
        }
        let patch = ModelPatch {
            name: model_name.clone(),
            ops: vec![
                ModelOperation::UpsertFlow(flow("back", "1")),
                ModelOperation::UpdateStockFlows {
                    ident: "a".to_string(),
                    inflows: vec!["back".to_string()],
                    outflows: vec!["f1".to_string()],
                },
                ModelOperation::UpdateStockFlows {
                    ident: "b".to_string(),
                    inflows: vec!["f1".to_string()],
                    outflows: vec!["back".to_string()],
                },
            ],
        };
        let new = incremental_layout(old, &patched, &model_name, &patch, None)
            .expect("incremental layout");
        assert_preserved(old, &new, &["f1"], &row);
        assert_eq!(violations_of(&new, "back"), Vec::<String>::new(), "{row}");
        assert_slot_keeps_spacing(&new, "back", &row);
        let back = find_flow(&new, "back").unwrap();
        let on = back
            .points
            .windows(2)
            .find(|w| {
                let (a, b) = (&w[0], &w[1]);
                let (lo_x, hi_x) = (a.x.min(b.x), a.x.max(b.x));
                let (lo_y, hi_y) = (a.y.min(b.y), a.y.max(b.y));
                back.x >= lo_x - 1e-6
                    && back.x <= hi_x + 1e-6
                    && back.y >= lo_y - 1e-6
                    && back.y <= hi_y + 1e-6
            })
            .unwrap_or_else(|| panic!("{row}: back's valve is off its pipe"));
        let len = (on[1].x - on[0].x).hypot(on[1].y - on[0].y);
        let from_ends = (back.x - on[0].x)
            .hypot(back.y - on[0].y)
            .min((back.x - on[1].x).hypot(back.y - on[1].y));
        assert!(
            len < 2.0 * VALVE_CLAMP_MARGIN || from_ends >= VALVE_CLAMP_MARGIN - 1e-6,
            "{row}: back's valve ({}, {}) is {from_ends} from an end of its {len}px segment",
            back.x,
            back.y
        );
        let f1 = find_flow(&new, "f1").unwrap();
        let (a, b) = (&f1.points[0], &f1.points[f1.points.len() - 1]);
        let (lo_x, hi_x) = (a.x.min(b.x), a.x.max(b.x));
        let off_f1 = (back.x - back.x.clamp(lo_x, hi_x)).hypot(back.y - a.y);
        assert!(
            off_f1 >= VALVE_CLAMP_MARGIN - 1e-6,
            "{row}: back's valve ({}, {}) is {off_f1} from f1's pipe",
            back.x,
            back.y
        );
    }
}

/// The old view breaks the invariants on every flow the patch does not touch,
/// in each way a pass could "repair": waste_a's valve is off its pipe and its
/// stock end is off stock_a's faces (the endpoint snap), chain_flow's pipe is
/// diagonal (the orthogonalizer), and waste_b's valve sits on its pipe 4px from
/// an end (the laid-out valve settle). Stored geometry like that -- a hand edit,
/// or a view saved before the invariants held -- is exactly what no pass may
/// move on a flow the patch did not touch, with a new element (the settle
/// path, which runs the snap and the finishing pass) and without one (the
/// early-return path).
#[test]
fn a_flow_the_patch_does_not_touch_is_preserved() {
    let base = project_of(vec![
        stock("stock_a", &[], &["chain_flow", "waste_a"]),
        stock("stock_b", &["chain_flow"], &["waste_b"]),
        datamodel::Variable::Flow(flow("chain_flow", "10")),
        datamodel::Variable::Flow(flow("waste_a", "stock_a * leak_rate")),
        datamodel::Variable::Flow(flow("waste_b", "1")),
        datamodel::Variable::Aux(aux("leak_rate", "0.1")),
    ]);
    let mut old = generate_layout(&base, TEST_MODEL, None).expect("initial layout");
    let stock_a_uid = stock_named(&old, "stock_a").uid;
    for e in &mut old.elements {
        if let ViewElement::Flow(f) = e
            && canonicalize(&f.name) == "waste_b"
        {
            let (a, b) = (&f.points[0], &f.points[1]);
            let len = (b.x - a.x).hypot(b.y - a.y);
            (f.x, f.y) = (a.x + (b.x - a.x) * 4.0 / len, a.y + (b.y - a.y) * 4.0 / len);
        }
        if let ViewElement::Flow(f) = e
            && canonicalize(&f.name) == "waste_a"
        {
            f.x += 30.0;
            for p in &mut f.points {
                if p.attached_to_uid == Some(stock_a_uid) {
                    p.x += 4.0;
                    p.y -= 5.0;
                }
            }
        }
        if let ViewElement::Flow(f) = e
            && canonicalize(&f.name) == "chain_flow"
        {
            let last = f.points.len() - 1;
            f.points[last].y += 6.0;
        }
    }
    let waste_problems = violations_of(&old, "waste_a");
    assert!(
        waste_problems.iter().any(|v| v.contains("off the pipe"))
            && waste_problems.iter().any(|v| v.contains("off the faces")),
        "fixture: waste_a's valve is off its pipe and its stock end off the faces: {waste_problems:?}"
    );
    assert!(
        violations_of(&old, "chain_flow")
            .iter()
            .any(|v| v.contains("diagonal")),
        "fixture: chain_flow's pipe is diagonal"
    );
    assert!(
        violations_of(&old, "waste_b")
            .iter()
            .any(|v| v.contains("from an end of the path")),
        "fixture: waste_b's valve is on its pipe within the margin of an end"
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
        &["waste_a", "chain_flow", "waste_b"],
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
        &["waste_a", "chain_flow", "waste_b"],
        "an unrelated edit, no new element",
    );
}
