// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The audit's decision table: one row per `FindingKind`.
//!
//! Every row starts from an edit applied and synced through the production
//! path (`build_scenario`, `apply_patch`, `sync_view`) on a hand-drawn view,
//! then makes the one change to the synced view the finding exists for. A row
//! checks both arms: the view as a correct sync produces it raises no finding
//! of that kind, and the changed view does. Where the correct view is not what
//! today's sync produces (a rebuilt element that moved, a pipe through a
//! stock), the row builds the correct arm by hand, stated in the row, so the
//! row does not depend on the defect the finding reports.

use super::*;
use crate::datamodel::view_element::{self, LabelSide};
use crate::layout::edit_scenarios::{
    Scenario, ScenarioKind, build_scenario, first_difference, run_scenario, sync_view,
};
use crate::patch::{ProjectPatch, apply_patch};

const MODEL: &str = "main";
const POPULATION: &str = "default_projects/population/model.xmile";
const SIR: &str = "test/test-models/samples/SIR/SIR.stmx";

fn load(rel: &str) -> datamodel::Project {
    let path = format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    crate::compat::open_xmile(&mut std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("{path}: {e:?}"))
}

fn shipped_view(project: &datamodel::Project) -> StockFlow {
    match project.get_model(MODEL).and_then(|m| m.views.first()) {
        Some(datamodel::View::StockFlow(sf)) => sf.clone(),
        None => panic!("no view"),
    }
}

/// One edit, applied and synced through the production path.
struct Edited {
    before: datamodel::Project,
    before_view: StockFlow,
    patch: ModelPatch,
    after: datamodel::Project,
    after_view: StockFlow,
}

fn edited(rel: &str, kind: ScenarioKind) -> Edited {
    edited_from(rel, kind, |_| {})
}

/// `edited`, from the shipped view changed by `prepare` first.
fn edited_from(rel: &str, kind: ScenarioKind, prepare: impl FnOnce(&mut StockFlow)) -> Edited {
    let mut before = load(rel);
    let mut before_view = shipped_view(&before);
    prepare(&mut before_view);
    before.get_model_mut(MODEL).expect("model").views =
        vec![datamodel::View::StockFlow(before_view.clone())];
    let scenario = build_scenario(&before, MODEL, kind).expect("the scenario applies");
    let patch = ModelPatch {
        name: before.get_model(MODEL).expect("model").name.clone(),
        ops: scenario.steps[0].clone(),
    };
    let mut after = before.clone();
    apply_patch(
        &mut after,
        ProjectPatch {
            project_ops: vec![],
            models: vec![patch.clone()],
        },
    )
    .expect("the patch applies");
    let after_view = sync_view(&after, MODEL, &patch, &before_view).expect("the sync succeeds");
    Edited {
        before,
        before_view,
        patch,
        after,
        after_view,
    }
}

impl Edited {
    /// The audit of this edit with the synced view changed by `change`.
    fn audit(&self, change: impl FnOnce(&mut StockFlow)) -> EditAudit {
        let mut view = self.after_view.clone();
        change(&mut view);
        let mut after = self.after.clone();
        after.get_model_mut(MODEL).expect("model").views =
            vec![datamodel::View::StockFlow(view.clone())];
        audit_edit(&EditInput {
            model_name: MODEL,
            before: &self.before,
            before_view: &self.before_view,
            patch: &self.patch,
            after: &after,
            after_view: &view,
        })
    }

    /// Assert the finding is absent after `correct` and present after
    /// `defective`.
    fn row(
        &self,
        kind: FindingKind,
        correct: impl FnOnce(&mut StockFlow),
        defective: impl FnOnce(&mut StockFlow),
    ) {
        let clean = self.audit(correct);
        assert!(
            !clean.kinds().contains(&kind),
            "{}: raised on the correct view: {:?}",
            kind.name(),
            clean
                .findings
                .iter()
                .filter(|f| f.kind == kind)
                .map(|f| &f.subject)
                .collect::<Vec<_>>()
        );
        let dirty = self.audit(defective);
        assert!(
            dirty.kinds().contains(&kind),
            "{}: not raised on the defective view; raised {:?}",
            kind.name(),
            dirty.kinds()
        );
    }
}

fn unchanged(_: &mut StockFlow) {}

fn uid_named(view: &StockFlow, ident: &str) -> i32 {
    view.elements
        .iter()
        .find(|e| named_ident(e).as_deref() == Some(ident))
        .map(ViewElement::get_uid)
        .unwrap_or_else(|| panic!("{ident} is drawn"))
}

fn element_named<'a>(view: &'a mut StockFlow, ident: &str) -> &'a mut ViewElement {
    view.elements
        .iter_mut()
        .find(|e| named_ident(e).as_deref() == Some(ident))
        .unwrap_or_else(|| panic!("{ident} is drawn"))
}

fn next_uid(view: &StockFlow) -> i32 {
    view.elements
        .iter()
        .map(ViewElement::get_uid)
        .max()
        .unwrap_or(0)
        + 1
}

fn link_between(view: &StockFlow, from: &str, to: &str) -> i32 {
    let (f, t) = (uid_named(view, from), uid_named(view, to));
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Link(l) if l.from_uid == f && l.to_uid == t => Some(l.uid),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a link {from} -> {to}"))
}

fn set_center(e: &mut ViewElement, x: f64, y: f64) {
    match e {
        ViewElement::Aux(a) => (a.x, a.y) = (x, y),
        ViewElement::Stock(s) => (s.x, s.y) = (x, y),
        ViewElement::Module(m) => (m.x, m.y) = (x, y),
        ViewElement::Flow(f) => (f.x, f.y) = (x, y),
        _ => panic!("no center"),
    }
}

fn push_link(view: &mut StockFlow, from_uid: i32, to_uid: i32) {
    let uid = next_uid(view);
    view.elements.push(ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape: LinkShape::Straight,
        polarity: None,
    }));
}

fn row_for(kind: FindingKind) {
    match kind {
        FindingKind::DeletedElementRemains => {
            // Delete a parameter; the defective sync still draws it.
            let e = edited(POPULATION, ScenarioKind::DeleteParameter);
            let old = e
                .before_view
                .elements
                .iter()
                .find(|el| named_ident(el).as_deref() == Some("average_lifespan"))
                .cloned()
                .expect("average_lifespan is drawn");
            e.row(kind, unchanged, |v| v.elements.push(old));
        }
        FindingKind::UntouchedElementChanged => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            e.row(kind, unchanged, |v| {
                if let ViewElement::Aux(a) = element_named(v, "average_lifespan") {
                    a.x += 20.0;
                }
            });
        }
        FindingKind::UntouchedLinkChanged => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let link = link_between(&e.after_view, "average_lifespan", "deaths");
            e.row(kind, unchanged, |v| {
                for el in &mut v.elements {
                    if let ViewElement::Link(l) = el
                        && l.uid == link
                    {
                        l.shape = match l.shape {
                            LinkShape::Straight => LinkShape::Arc(30.0),
                            _ => LinkShape::Straight,
                        };
                    }
                }
            });
        }
        FindingKind::StaleLinkRemains => {
            // Delete a parameter; the defective sync keeps the link it drew.
            let e = edited(POPULATION, ScenarioKind::DeleteParameter);
            let old = link_between(&e.before_view, "average_lifespan", "deaths");
            let link = e
                .before_view
                .elements
                .iter()
                .find(|el| el.get_uid() == old)
                .cloned()
                .expect("the link");
            e.row(kind, unchanged, |v| v.elements.push(link));
        }
        FindingKind::UnrelatedLinkAdded => {
            // The author's view leaves out average_lifespan -> deaths. An edit
            // adding births_multiplier is not about that dependency, so the
            // correct view still leaves it out.
            let without = |v: &mut StockFlow| {
                let (from, to) = (uid_named(v, "average_lifespan"), uid_named(v, "deaths"));
                v.elements
                    .retain(|el| !matches!(el, ViewElement::Link(l) if l.from_uid == from && l.to_uid == to));
            };
            let e = edited_from(POPULATION, ScenarioKind::AddParameter, without);
            let (from, to) = (
                uid_named(&e.after_view, "average_lifespan"),
                uid_named(&e.after_view, "deaths"),
            );
            e.row(kind, without, |v| {
                without(v);
                push_link(v, from, to);
            });
        }
        FindingKind::RebuiltElementMoved => {
            // Turn a parameter into a stock. The correct arm puts the rebuilt
            // stock at the parameter's old center by hand.
            let e = edited(POPULATION, ScenarioKind::AuxToStock);
            let (x, y) = match e
                .before_view
                .elements
                .iter()
                .find(|el| named_ident(el).as_deref() == Some("average_lifespan"))
            {
                Some(ViewElement::Aux(a)) => (a.x, a.y),
                _ => panic!("average_lifespan is an aux"),
            };
            e.row(
                kind,
                |v| set_center(element_named(v, "average_lifespan"), x, y),
                |v| set_center(element_named(v, "average_lifespan"), x + 30.0, y),
            );
        }
        FindingKind::ViewPropertiesChanged => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            e.row(kind, unchanged, |v| v.zoom *= 2.0);
        }
        FindingKind::UidProblem => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let taken = uid_named(&e.after_view, "population");
            e.row(kind, unchanged, |v| {
                if let ViewElement::Aux(a) = element_named(v, "births_multiplier") {
                    a.uid = taken;
                }
            });
        }
        FindingKind::VariableNotDrawn => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let uid = uid_named(&e.after_view, "births_multiplier");
            e.row(kind, unchanged, |v| {
                v.elements.retain(|el| match el {
                    ViewElement::Link(l) => l.from_uid != uid && l.to_uid != uid,
                    other => other.get_uid() != uid,
                })
            });
        }
        FindingKind::VariableDrawnTwice => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            e.row(kind, unchanged, |v| {
                let mut copy = element_named(v, "births_multiplier").clone();
                if let ViewElement::Aux(a) = &mut copy {
                    a.uid = next_uid(v);
                    a.y += 80.0;
                }
                v.elements.push(copy);
            });
        }
        FindingKind::ElementKindMismatch => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            e.row(kind, unchanged, |v| {
                let el = element_named(v, "births_multiplier");
                if let ViewElement::Aux(a) = el.clone() {
                    *el = ViewElement::Stock(view_element::Stock {
                        name: a.name,
                        uid: a.uid,
                        x: a.x,
                        y: a.y,
                        label_side: LabelSide::Bottom,
                        compat: None,
                    });
                }
            });
        }
        FindingKind::ElementNamesNoVariable => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            e.row(kind, unchanged, |v| {
                let uid = next_uid(v);
                v.elements.push(ViewElement::Aux(view_element::Aux {
                    name: "phantom variable".to_string(),
                    uid,
                    x: 900.0,
                    y: 900.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                }));
            });
        }
        FindingKind::DanglingReference => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let births = uid_named(&e.after_view, "births");
            e.row(kind, unchanged, |v| push_link(v, 99_999, births));
        }
        FindingKind::LinkWithoutDependency => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let (from, to) = (
                uid_named(&e.after_view, "average_lifespan"),
                uid_named(&e.after_view, "births"),
            );
            e.row(kind, unchanged, |v| push_link(v, from, to));
        }
        FindingKind::DependencyWithoutLink => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let link = link_between(&e.after_view, "births_multiplier", "births");
            e.row(kind, unchanged, |v| {
                v.elements.retain(|el| el.get_uid() != link)
            });
        }
        FindingKind::FlowAttachmentMismatch => {
            // deaths drains population into a cloud; attach its cloud end to
            // population instead.
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let population = uid_named(&e.after_view, "population");
            e.row(kind, unchanged, |v| {
                if let ViewElement::Flow(f) = element_named(v, "deaths") {
                    f.points.last_mut().expect("points").attached_to_uid = Some(population);
                }
            });
        }
        FindingKind::FlowInvariant => {
            // Pull a created side flow's valve off its pipe (G8).
            let e = edited(POPULATION, ScenarioKind::AddSideFlow);
            e.row(kind, unchanged, |v| {
                if let ViewElement::Flow(f) = element_named(v, "population_loss") {
                    f.x += 40.0;
                    f.y += 40.0;
                }
            });
        }
        FindingKind::ShapeOverlap => {
            let e = edited(POPULATION, ScenarioKind::AddParameter);
            let (x, y) = match e
                .after_view
                .elements
                .iter()
                .find(|el| named_ident(el).as_deref() == Some("population"))
            {
                Some(ViewElement::Stock(s)) => (s.x, s.y),
                _ => panic!("population is a stock"),
            };
            e.row(kind, unchanged, |v| {
                set_center(element_named(v, "births_multiplier"), x, y)
            });
        }
        FindingKind::PipeThroughStock => {
            // A flow from recovered to susceptible, past infectious between
            // them. The correct arm routes it by hand above every stock; the
            // defective one runs it straight through infectious.
            let e = edited(SIR, ScenarioKind::AddFlowBetweenStocks);
            let stock = |ident: &str| match e
                .after_view
                .elements
                .iter()
                .find(|el| named_ident(el).as_deref() == Some(ident))
            {
                Some(ViewElement::Stock(s)) => (s.uid, s.x, s.y),
                _ => panic!("{ident} is a stock"),
            };
            let (r_uid, rx, ry) = stock("recovered");
            let (s_uid, sx, sy) = stock("susceptible");
            let half_h = crate::diagram::constants::STOCK_HEIGHT / 2.0;
            let half_w = crate::diagram::constants::STOCK_WIDTH / 2.0;
            let top = ry.min(sy) - 100.0;
            let point = |x: f64, y: f64, uid: Option<i32>| view_element::FlowPoint {
                x,
                y,
                attached_to_uid: uid,
            };
            let route =
                |v: &mut StockFlow, points: Vec<view_element::FlowPoint>, valve: (f64, f64)| {
                    if let ViewElement::Flow(f) = element_named(v, "recovered_to_susceptible") {
                        f.points = points;
                        (f.x, f.y) = valve;
                    }
                };
            e.row(
                kind,
                |v| {
                    route(
                        v,
                        vec![
                            point(rx, ry - half_h, Some(r_uid)),
                            point(rx, top, None),
                            point(sx, top, None),
                            point(sx, sy - half_h, Some(s_uid)),
                        ],
                        ((rx + sx) / 2.0, top),
                    )
                },
                |v| {
                    route(
                        v,
                        vec![
                            point(rx - half_w, ry, Some(r_uid)),
                            point(sx + half_w, sy, Some(s_uid)),
                        ],
                        ((rx + sx) / 2.0, (ry + sy) / 2.0),
                    )
                },
            );
        }
        FindingKind::NotDeterministic => {
            // The runner raises this exactly when `first_difference` between
            // two syncs of one edit is `Some`; a production sync is
            // deterministic, so the row pins the comparison's arms instead.
            let project = load(POPULATION);
            let view = shipped_view(&project);
            assert_eq!(first_difference(&view, &view), None);
            let mut moved = view.clone();
            if let ViewElement::Aux(a) = element_named(&mut moved, "birth_rate") {
                a.x += 1.0;
            }
            assert!(first_difference(&view, &moved).is_some());
            let mut reordered = view.clone();
            reordered.elements.reverse();
            assert!(first_difference(&view, &reordered).is_some());
            let mut rezoomed = view.clone();
            rezoomed.zoom *= 2.0;
            assert!(first_difference(&view, &rezoomed).is_some());
        }
        FindingKind::ReturnToOriginal => {
            let project = load(POPULATION);
            let view = shipped_view(&project);
            let restate =
                build_scenario(&project, MODEL, ScenarioKind::RestateVariable).expect("applies");
            assert!(
                !run_scenario(&project, MODEL, &view, &restate)
                    .kinds()
                    .contains(&kind)
            );
            let adds =
                build_scenario(&project, MODEL, ScenarioKind::AddParameter).expect("applies");
            let claims_identity = Scenario {
                returns_to_original: true,
                ..adds
            };
            assert!(
                run_scenario(&project, MODEL, &view, &claims_identity)
                    .kinds()
                    .contains(&kind)
            );
        }
        FindingKind::SyncFailed => {
            let project = load(POPULATION);
            let view = shipped_view(&project);
            let restate =
                build_scenario(&project, MODEL, ScenarioKind::RestateVariable).expect("applies");
            assert!(
                !run_scenario(&project, MODEL, &view, &restate)
                    .kinds()
                    .contains(&kind)
            );
            let broken = Scenario {
                steps: vec![vec![ModelOperation::DeleteVariable {
                    ident: "no_such_variable".to_string(),
                }]],
                ..restate
            };
            assert!(
                run_scenario(&project, MODEL, &view, &broken)
                    .kinds()
                    .contains(&kind)
            );
        }
    }
}

#[test]
fn every_finding_kind_is_raised_exactly_where_it_applies() {
    for kind in FindingKind::ALL {
        row_for(kind);
    }
}

#[test]
fn an_edit_that_changes_nothing_raises_nothing() {
    let project = load(POPULATION);
    let view = shipped_view(&project);
    let patch = ModelPatch {
        name: project.get_model(MODEL).expect("model").name.clone(),
        ops: vec![],
    };
    let audit = audit_edit(&EditInput {
        model_name: MODEL,
        before: &project,
        before_view: &view,
        patch: &patch,
        after: &project,
        after_view: &view,
    });
    assert!(
        audit.findings.is_empty(),
        "{:?}",
        audit
            .findings
            .iter()
            .map(|f| (&f.kind, &f.subject))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_rename_is_charged_nothing_for_the_new_name() {
    // Findings key on the ident after the edit, so an imported view's
    // pre-existing inconsistency about a renamed variable is not charged to
    // the rename, and the renamed element's name is its one allowed change.
    let e = edited(POPULATION, ScenarioKind::RenameVariable);
    let audit = e.audit(unchanged);
    assert!(
        audit.findings.is_empty(),
        "{:?}",
        audit
            .findings
            .iter()
            .map(|f| (&f.kind, &f.subject))
            .collect::<Vec<_>>()
    );
}
