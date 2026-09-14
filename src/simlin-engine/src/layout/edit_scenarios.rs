// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Edit scenarios: the edits an agent or a notebook user makes to a model,
//! generated for any model, driven through the production patch and sync path,
//! and audited step by step (`layout::edit_audit`).
//!
//! Every `ScenarioKind` picks its targets from the model deterministically
//! (the first candidate by ident) and is not applicable to a model that has
//! none. A scenario is a sequence of patches, each applied with `apply_patch`
//! to a project holding the current view and synced by `sync_view`, the rule
//! MCP `edit_model` and libsimlin's patch sync follow.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::common::canonicalize;
use crate::datamodel::{self, Equation, StockFlow, Variable, ViewElement};
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};

use super::edit_audit::{Displacement, EditAudit, EditInput, Finding, FindingKind, audit_edit};
use super::metadata::ComputedMetadata;
use super::{compute_dependency_metadata, generate_best_layout, incremental_layout};

/// One kind of edit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    /// Upsert a variable exactly as it is, as an agent restating a definition
    /// does: the view must not change.
    RestateVariable,
    /// Add a parameter a flow's equation multiplies by.
    AddParameter,
    /// Put a new variable between a parameter and a variable that reads it.
    InsertIntermediate,
    /// Delete a parameter.
    DeleteParameter,
    /// Delete a flow.
    DeleteFlow,
    /// Delete a stock that has both inflows and outflows.
    DeleteMiddleStock,
    /// Restate a stock with one of its flows left out of its lists.
    DetachFlow,
    /// Turn a parameter into a stock.
    AuxToStock,
    /// Rename a parameter with the rename operation.
    RenameVariable,
    /// Rename a parameter the way an agent without a rename operation does:
    /// delete it, create it under the new name, and rewrite its readers.
    RenameByRemoveAndAdd,
    /// Add a flow between two stocks no flow joins.
    AddFlowBetweenStocks,
    /// Make a parameter read a stock it feeds, closing a feedback loop.
    CloseLoop,
    /// Add a stock downstream of a stock, joined by a new flow.
    ExtendChain,
    /// Add an outflow from a stock to a cloud.
    AddSideFlow,
    /// Add a disconnected stock with an inflow, an outflow, and a parameter.
    AddSector,
    /// Add a parameter, then delete it and restore the flow: the view must
    /// come back to where it started.
    AddThenUndo,
}

impl ScenarioKind {
    pub const ALL: [ScenarioKind; 16] = [
        ScenarioKind::RestateVariable,
        ScenarioKind::AddParameter,
        ScenarioKind::InsertIntermediate,
        ScenarioKind::DeleteParameter,
        ScenarioKind::DeleteFlow,
        ScenarioKind::DeleteMiddleStock,
        ScenarioKind::DetachFlow,
        ScenarioKind::AuxToStock,
        ScenarioKind::RenameVariable,
        ScenarioKind::RenameByRemoveAndAdd,
        ScenarioKind::AddFlowBetweenStocks,
        ScenarioKind::CloseLoop,
        ScenarioKind::ExtendChain,
        ScenarioKind::AddSideFlow,
        ScenarioKind::AddSector,
        ScenarioKind::AddThenUndo,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ScenarioKind::RestateVariable => "restate_variable",
            ScenarioKind::AddParameter => "add_parameter",
            ScenarioKind::InsertIntermediate => "insert_intermediate",
            ScenarioKind::DeleteParameter => "delete_parameter",
            ScenarioKind::DeleteFlow => "delete_flow",
            ScenarioKind::DeleteMiddleStock => "delete_middle_stock",
            ScenarioKind::DetachFlow => "detach_flow",
            ScenarioKind::AuxToStock => "aux_to_stock",
            ScenarioKind::RenameVariable => "rename_variable",
            ScenarioKind::RenameByRemoveAndAdd => "rename_by_remove_and_add",
            ScenarioKind::AddFlowBetweenStocks => "add_flow_between_stocks",
            ScenarioKind::CloseLoop => "close_loop",
            ScenarioKind::ExtendChain => "extend_chain",
            ScenarioKind::AddSideFlow => "add_side_flow",
            ScenarioKind::AddSector => "add_sector",
            ScenarioKind::AddThenUndo => "add_then_undo",
        }
    }
}

/// A generated edit sequence for one model.
#[derive(Clone)]
pub struct Scenario {
    pub kind: ScenarioKind,
    /// What the scenario does to this model, naming its targets.
    pub description: String,
    /// The operations of each patch, in order.
    pub steps: Vec<Vec<ModelOperation>>,
    /// `(ident before, ident after)` for a variable an edit gives a new
    /// identity (a rename spelled as a delete and a create): how far it moved
    /// is reported, since no audit can tell they are one variable.
    pub continuity: Vec<(String, String)>,
    /// Whether the last step must leave the view exactly as it began.
    pub returns_to_original: bool,
}

/// One applied and synced patch.
pub struct StepOutcome {
    pub before_view: StockFlow,
    pub after: datamodel::Project,
    pub after_view: StockFlow,
    pub audit: EditAudit,
}

/// A scenario run.
pub struct ScenarioOutcome {
    pub kind: ScenarioKind,
    pub description: String,
    pub steps: Vec<StepOutcome>,
    /// Every step's findings, then the runner's own.
    pub findings: Vec<Finding>,
    pub continuity: Vec<Displacement>,
}

impl ScenarioOutcome {
    pub fn kinds(&self) -> BTreeSet<FindingKind> {
        self.findings.iter().map(|f| f.kind).collect()
    }
}

/// Sync `old_view` to `project`, which `patch` has already been applied to:
/// a full layout while the view is empty, the incremental layout after that,
/// keeping the view's zoom (the rule MCP `edit_model` and
/// `simlin_project_diagram_sync` follow).
pub fn sync_view(
    project: &datamodel::Project,
    model_name: &str,
    patch: &ModelPatch,
    old_view: &StockFlow,
) -> Result<StockFlow, String> {
    let mut view = if old_view.elements.is_empty() {
        generate_best_layout(project, model_name, None)?
    } else {
        incremental_layout(old_view, project, model_name, patch, None)?
    };
    if old_view.zoom > 0.0 {
        view.zoom = old_view.zoom;
    }
    Ok(view)
}

fn with_view(
    project: &datamodel::Project,
    model_name: &str,
    view: &StockFlow,
) -> datamodel::Project {
    let mut p = project.clone();
    if let Some(m) = p.get_model_mut(model_name) {
        m.views = vec![datamodel::View::StockFlow(view.clone())];
    }
    p
}

/// The first way `b` differs from `a`, or `None` when they are equal.
pub fn first_difference(a: &StockFlow, b: &StockFlow) -> Option<String> {
    if a == b {
        return None;
    }
    let props = |v: &StockFlow| {
        (
            v.name.clone(),
            v.view_box.clone(),
            v.zoom,
            v.use_lettered_polarity,
            v.font.clone(),
        )
    };
    if props(a) != props(b) {
        return Some("view properties differ".to_string());
    }
    let by_uid = |v: &StockFlow| -> BTreeMap<i32, ViewElement> {
        v.elements
            .iter()
            .map(|e| (e.get_uid(), e.clone()))
            .collect()
    };
    let (ea, eb) = (by_uid(a), by_uid(b));
    for (uid, x) in &ea {
        match eb.get(uid) {
            None => return Some(format!("element #{uid} missing")),
            Some(y) if y != x => return Some(format!("element #{uid} differs")),
            _ => {}
        }
    }
    if let Some(uid) = eb.keys().find(|u| !ea.contains_key(u)) {
        return Some(format!("element #{uid} added"));
    }
    Some("element order differs".to_string())
}

/// Drive `scenario` from `view` on `project`, auditing every step.
pub fn run_scenario(
    project: &datamodel::Project,
    model_name: &str,
    view: &StockFlow,
    scenario: &Scenario,
) -> ScenarioOutcome {
    let mut outcome = ScenarioOutcome {
        kind: scenario.kind,
        description: scenario.description.clone(),
        steps: Vec::new(),
        findings: Vec::new(),
        continuity: Vec::new(),
    };
    let Some(patch_name) = project.get_model(model_name).map(|m| m.name.clone()) else {
        outcome.findings.push(Finding::new(
            FindingKind::SyncFailed,
            model_name,
            "no such model",
            None,
        ));
        return outcome;
    };
    let mut current = with_view(project, model_name, view);
    let mut current_view = view.clone();
    let mut runner: Vec<Finding> = Vec::new();
    for ops in &scenario.steps {
        let patch = ModelPatch {
            name: patch_name.clone(),
            ops: ops.clone(),
        };
        let mut after = current.clone();
        if let Err(err) = apply_patch(
            &mut after,
            ProjectPatch {
                project_ops: vec![],
                models: vec![patch.clone()],
            },
        ) {
            runner.push(Finding::new(
                FindingKind::SyncFailed,
                model_name,
                format!("patch failed: {err}"),
                None,
            ));
            break;
        }
        let after_view = match sync_view(&after, model_name, &patch, &current_view) {
            Ok(v) => v,
            Err(err) => {
                runner.push(Finding::new(
                    FindingKind::SyncFailed,
                    model_name,
                    format!("sync failed: {err}"),
                    None,
                ));
                break;
            }
        };
        match sync_view(&after, model_name, &patch, &current_view) {
            Ok(again) => {
                if let Some(diff) = first_difference(&after_view, &again) {
                    runner.push(Finding::new(
                        FindingKind::NotDeterministic,
                        model_name,
                        diff,
                        None,
                    ));
                }
            }
            Err(err) => runner.push(Finding::new(
                FindingKind::NotDeterministic,
                model_name,
                format!("a second sync failed: {err}"),
                None,
            )),
        }
        let after = with_view(&after, model_name, &after_view);
        let audit = audit_edit(&EditInput {
            model_name,
            before: &current,
            before_view: &current_view,
            patch: &patch,
            after: &after,
            after_view: &after_view,
        });
        outcome.findings.extend(audit.findings.iter().cloned());
        outcome.steps.push(StepOutcome {
            before_view: current_view.clone(),
            after: after.clone(),
            after_view: after_view.clone(),
            audit,
        });
        current = after;
        current_view = after_view;
    }
    let completed = outcome.steps.len() == scenario.steps.len();
    if scenario.returns_to_original
        && completed
        && let Some(diff) = first_difference(view, &current_view)
    {
        runner.push(Finding::new(
            FindingKind::ReturnToOriginal,
            model_name,
            diff,
            None,
        ));
    }
    let center_named = |v: &StockFlow, ident: &str| {
        v.elements.iter().find_map(|e| {
            let name = e.get_name()?;
            if canonicalize(name) != ident {
                return None;
            }
            match e {
                ViewElement::Aux(a) => Some((a.x, a.y)),
                ViewElement::Stock(s) => Some((s.x, s.y)),
                ViewElement::Flow(f) => Some((f.x, f.y)),
                ViewElement::Module(m) => Some((m.x, m.y)),
                _ => None,
            }
        })
    };
    if completed {
        for (old, new) in &scenario.continuity {
            if let (Some(a), Some(b)) = (center_named(view, old), center_named(&current_view, new))
            {
                outcome.continuity.push(Displacement {
                    subject: format!("{old} -> {new}"),
                    distance: (a.0 - b.0).hypot(a.1 - b.1),
                });
            }
        }
    }
    outcome.findings.extend(runner);
    outcome
}

fn scalar(v: &Variable) -> Option<&str> {
    match v.get_equation()? {
        Equation::Scalar(s) => Some(s.as_str()),
        _ => None,
    }
}

fn has_table(v: &Variable) -> bool {
    match v {
        Variable::Aux(a) => a.gf.is_some(),
        Variable::Flow(f) => f.gf.is_some(),
        _ => false,
    }
}

fn aux(ident: &str, equation: &str) -> ModelOperation {
    ModelOperation::UpsertAux(datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn flow(ident: &str, equation: &str) -> ModelOperation {
    ModelOperation::UpsertFlow(datamodel::Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn stock(ident: &str, equation: &str, inflows: &[String], outflows: &[String]) -> ModelOperation {
    ModelOperation::UpsertStock(datamodel::Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.to_vec(),
        outflows: outflows.to_vec(),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn upsert(v: Variable) -> ModelOperation {
    match v {
        Variable::Stock(s) => ModelOperation::UpsertStock(s),
        Variable::Flow(f) => ModelOperation::UpsertFlow(f),
        Variable::Aux(a) => ModelOperation::UpsertAux(a),
        Variable::Module(m) => ModelOperation::UpsertModule(m),
    }
}

/// The model's variables and the dependencies a diagram draws for them, with
/// the target choices every scenario picks from.
struct Targets<'a> {
    project: &'a datamodel::Project,
    model_name: &'a str,
    patch_name: String,
    vars: BTreeMap<String, &'a Variable>,
    /// The variables the view draws, which are the ones scenarios target: a
    /// variable the view does not draw is drawn by any edit that names it, so
    /// a scenario about one exercises that rule instead of the edit, and an
    /// edit expected to return the original view cannot. `None` when the model
    /// has no view.
    drawn: Option<BTreeSet<String>>,
    meta: ComputedMetadata,
}

impl<'a> Targets<'a> {
    fn new(project: &'a datamodel::Project, model_name: &'a str) -> Option<Targets<'a>> {
        let model = project.get_model(model_name)?;
        let meta = compute_dependency_metadata(project, model_name, None)?;
        let drawn = model
            .views
            .iter()
            .map(|v| match v {
                datamodel::View::StockFlow(sf) => sf
                    .elements
                    .iter()
                    .filter(|e| {
                        matches!(
                            e,
                            ViewElement::Stock(_)
                                | ViewElement::Flow(_)
                                | ViewElement::Aux(_)
                                | ViewElement::Module(_)
                        )
                    })
                    .filter_map(|e| e.get_name().map(|n| canonicalize(n).into_owned()))
                    .collect::<BTreeSet<String>>(),
            })
            .next();
        Some(Targets {
            project,
            model_name,
            patch_name: model.name.clone(),
            drawn,
            vars: model
                .variables
                .iter()
                .map(|v| (canonicalize(v.get_ident()).into_owned(), v))
                .collect(),
            meta,
        })
    }

    fn var(&self, ident: &str) -> Option<&'a Variable> {
        self.vars.get(ident).copied()
    }

    fn is_drawn(&self, ident: &str) -> bool {
        self.drawn.as_ref().is_none_or(|d| d.contains(ident))
    }

    /// The drawn variables that read `ident`.
    fn dependents(&self, ident: &str) -> Vec<String> {
        self.meta
            .reverse_dep_graph
            .get(ident)
            .map(|s| {
                s.iter()
                    .filter(|d| *d != ident && self.is_drawn(d))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Scalar auxes with no reads and at least one reader, by ident.
    fn parameters(&self) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(ident, v)| {
                self.is_drawn(ident)
                    && matches!(v, Variable::Aux(_))
                    && scalar(v).is_some()
                    && !has_table(v)
                    && self.meta.dep_graph.get(*ident).is_none_or(|d| d.is_empty())
                    && !self.dependents(ident).is_empty()
            })
            .map(|(ident, _)| ident.clone())
            .collect()
    }

    fn parameter(&self) -> Option<String> {
        self.parameters().into_iter().next()
    }

    fn flows(&self) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(ident, v)| {
                self.is_drawn(ident)
                    && matches!(v, Variable::Flow(_))
                    && scalar(v).is_some()
                    && !has_table(v)
            })
            .map(|(ident, _)| ident.clone())
            .collect()
    }

    fn stocks(&self) -> Vec<(String, &'a datamodel::Stock)> {
        self.vars
            .iter()
            .filter_map(|(ident, v)| match v {
                Variable::Stock(s) if self.is_drawn(ident) && scalar(v).is_some() => {
                    Some((ident.clone(), s))
                }
                _ => None,
            })
            .collect()
    }

    /// `base`, or `base` with a number appended, naming no variable.
    fn fresh(&self, base: &str) -> String {
        let base = canonicalize(base).into_owned();
        if !self.vars.contains_key(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}_{n}"))
            .find(|name| !self.vars.contains_key(name))
            .expect("an unused name")
    }

    fn with_equation(&self, ident: &str, equation: &str) -> Option<ModelOperation> {
        let mut v = self.var(ident)?.clone();
        v.set_scalar_equation(equation);
        Some(upsert(v))
    }

    fn with_flows(
        &self,
        ident: &str,
        inflows: Vec<String>,
        outflows: Vec<String>,
    ) -> Option<ModelOperation> {
        let Variable::Stock(mut s) = self.var(ident)?.clone() else {
            return None;
        };
        s.inflows = inflows;
        s.outflows = outflows;
        Some(ModelOperation::UpsertStock(s))
    }

    /// `reader`'s equation with every reference to `from` spelled `to`, by the
    /// rename operation's own rewriting.
    fn renamed_equation(&self, reader: &str, from: &str, to: &str) -> Option<String> {
        let mut p = self.project.clone();
        apply_patch(
            &mut p,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: self.patch_name.clone(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: from.to_string(),
                        to: to.to_string(),
                    }],
                }],
            },
        )
        .ok()?;
        let model = p.get_model(self.model_name)?;
        let v = model
            .variables
            .iter()
            .find(|v| canonicalize(v.get_ident()) == reader)?;
        scalar(v).map(str::to_string)
    }

    fn add_parameter(&self) -> Option<(String, String, Vec<ModelOperation>)> {
        let f = self.flows().into_iter().next()?;
        let eqn = scalar(self.var(&f)?)?;
        let n = self.fresh(&format!("{f}_multiplier"));
        let ops = vec![
            aux(&n, "1"),
            self.with_equation(&f, &format!("({eqn}) * {n}"))?,
        ];
        Some((f, n, ops))
    }
}

/// The scenario of `kind` for `project`'s model, or `None` when the model has
/// nothing it applies to.
pub fn build_scenario(
    project: &datamodel::Project,
    model_name: &str,
    kind: ScenarioKind,
) -> Option<Scenario> {
    let t = Targets::new(project, model_name)?;
    let single = |description: String, ops: Vec<ModelOperation>| Scenario {
        kind,
        description,
        steps: vec![ops],
        continuity: Vec::new(),
        returns_to_original: false,
    };
    match kind {
        ScenarioKind::RestateVariable => {
            let ident = t
                .flows()
                .into_iter()
                .next()
                .or_else(|| t.parameter())
                .or_else(|| t.stocks().into_iter().next().map(|(i, _)| i))?;
            Some(Scenario {
                returns_to_original: true,
                ..single(
                    format!("restate {ident}"),
                    vec![upsert(t.var(&ident)?.clone())],
                )
            })
        }
        ScenarioKind::AddParameter => {
            let (f, n, ops) = t.add_parameter()?;
            Some(single(format!("add {n}, read by {f}"), ops))
        }
        ScenarioKind::InsertIntermediate => {
            let (p, reader) = t.parameters().into_iter().find_map(|p| {
                let reader = t.dependents(&p).into_iter().find(|r| {
                    t.var(r).is_some_and(|v| {
                        matches!(v, Variable::Aux(_) | Variable::Flow(_)) && scalar(v).is_some()
                    })
                })?;
                Some((p, reader))
            })?;
            let x = t.fresh(&format!("{p}_effective"));
            let eqn = t.renamed_equation(&reader, &p, &x)?;
            Some(single(
                format!("insert {x} between {p} and {reader}"),
                vec![aux(&x, &p), t.with_equation(&reader, &eqn)?],
            ))
        }
        ScenarioKind::DeleteParameter => {
            let p = t.parameter()?;
            Some(single(
                format!("delete {p}"),
                vec![ModelOperation::DeleteVariable { ident: p }],
            ))
        }
        ScenarioKind::DeleteFlow => {
            let f = t.flows().into_iter().next()?;
            Some(single(
                format!("delete {f}"),
                vec![ModelOperation::DeleteVariable { ident: f }],
            ))
        }
        ScenarioKind::DeleteMiddleStock => {
            let (s, _) = t
                .stocks()
                .into_iter()
                .find(|(_, s)| !s.inflows.is_empty() && !s.outflows.is_empty())?;
            Some(single(
                format!("delete {s}"),
                vec![ModelOperation::DeleteVariable { ident: s }],
            ))
        }
        ScenarioKind::DetachFlow => {
            let (s, st) = t
                .stocks()
                .into_iter()
                .find(|(_, s)| !s.outflows.is_empty() || !s.inflows.is_empty())?;
            let (mut inflows, mut outflows) = (st.inflows.clone(), st.outflows.clone());
            let detached = if outflows.is_empty() {
                inflows.remove(0)
            } else {
                outflows.remove(0)
            };
            Some(single(
                format!("restate {s} without {detached}"),
                vec![t.with_flows(&s, inflows, outflows)?],
            ))
        }
        ScenarioKind::AuxToStock => {
            let p = t.parameter()?;
            let Variable::Aux(a) = t.var(&p)? else {
                return None;
            };
            Some(single(
                format!("turn {p} into a stock"),
                vec![ModelOperation::UpsertStock(datamodel::Stock {
                    ident: a.ident.clone(),
                    equation: a.equation.clone(),
                    documentation: a.documentation.clone(),
                    units: a.units.clone(),
                    inflows: Vec::new(),
                    outflows: Vec::new(),
                    ai_state: None,
                    uid: a.uid,
                    compat: datamodel::Compat::default(),
                })],
            ))
        }
        ScenarioKind::RenameVariable => {
            let p = t.parameter()?;
            let to = t.fresh(&format!("{p}_renamed"));
            Some(single(
                format!("rename {p} to {to}"),
                vec![ModelOperation::RenameVariable {
                    from: t.var(&p)?.get_ident().to_string(),
                    to,
                }],
            ))
        }
        ScenarioKind::RenameByRemoveAndAdd => {
            let p = t.parameter()?;
            let eqn = scalar(t.var(&p)?)?.to_string();
            let to = t.fresh(&format!("{p}_renamed"));
            let mut ops = vec![
                ModelOperation::DeleteVariable {
                    ident: t.var(&p)?.get_ident().to_string(),
                },
                aux(&to, &eqn),
            ];
            for reader in t.dependents(&p) {
                let renamed = t.renamed_equation(&reader, &p, &to)?;
                ops.push(t.with_equation(&reader, &renamed)?);
            }
            Some(Scenario {
                continuity: vec![(p.clone(), to.clone())],
                ..single(format!("rename {p} to {to} by delete and create"), ops)
            })
        }
        ScenarioKind::AddFlowBetweenStocks => {
            let stocks = t.stocks();
            let joined = |a: &str, b: &str| {
                t.meta.flow_to_stocks.values().any(|(from, to)| {
                    let (from, to) = (from.as_deref(), to.as_deref());
                    (from == Some(a) && to == Some(b)) || (from == Some(b) && to == Some(a))
                })
            };
            let (a, sa, b, sb) = stocks.iter().enumerate().find_map(|(i, (a, sa))| {
                stocks[i + 1..]
                    .iter()
                    .find(|(b, _)| !joined(a, b))
                    .map(|(b, sb)| (a.clone(), *sa, b.clone(), *sb))
            })?;
            let n = t.fresh(&format!("{a}_to_{b}"));
            let mut out_a = sa.outflows.clone();
            out_a.push(n.clone());
            let mut in_b = sb.inflows.clone();
            in_b.push(n.clone());
            Some(single(
                format!("add {n} from {a} to {b}"),
                vec![
                    flow(&n, &format!("{a} * 0.01")),
                    t.with_flows(&a, sa.inflows.clone(), out_a)?,
                    t.with_flows(&b, in_b, sb.outflows.clone())?,
                ],
            ))
        }
        ScenarioKind::CloseLoop => {
            let (p, s) = t.parameters().into_iter().find_map(|p| {
                let mut seen: BTreeSet<String> = BTreeSet::new();
                let mut queue: VecDeque<String> = VecDeque::from([p.clone()]);
                let mut reached: BTreeSet<String> = BTreeSet::new();
                while let Some(ident) = queue.pop_front() {
                    for next in t.dependents(&ident) {
                        if seen.insert(next.clone()) {
                            if matches!(t.var(&next), Some(Variable::Stock(_))) {
                                reached.insert(next.clone());
                            }
                            queue.push_back(next);
                        }
                    }
                }
                reached.into_iter().next().map(|s| (p, s))
            })?;
            let eqn = scalar(t.var(&p)?)?;
            Some(single(
                format!("make {p} read {s}"),
                vec![t.with_equation(&p, &format!("({eqn}) * (1 + {s} / 1000)"))?],
            ))
        }
        ScenarioKind::ExtendChain => {
            let (s, st) = t.stocks().into_iter().next()?;
            let downstream = t.fresh(&format!("{s}_downstream"));
            let transfer = t.fresh(&format!("{s}_transfer"));
            let mut outflows = st.outflows.clone();
            outflows.push(transfer.clone());
            Some(single(
                format!("add {downstream}, fed from {s} by {transfer}"),
                vec![
                    flow(&transfer, &format!("{s} * 0.1")),
                    stock(&downstream, "0", std::slice::from_ref(&transfer), &[]),
                    t.with_flows(&s, st.inflows.clone(), outflows)?,
                ],
            ))
        }
        ScenarioKind::AddSideFlow => {
            let (s, st) = t.stocks().into_iter().next()?;
            let loss = t.fresh(&format!("{s}_loss"));
            let mut outflows = st.outflows.clone();
            outflows.push(loss.clone());
            Some(single(
                format!("add {loss} out of {s}"),
                vec![
                    flow(&loss, &format!("{s} * 0.01")),
                    t.with_flows(&s, st.inflows.clone(), outflows)?,
                ],
            ))
        }
        ScenarioKind::AddSector => {
            let level = t.fresh("edit_sector_stock");
            let inflow = t.fresh("edit_sector_inflow");
            let outflow = t.fresh("edit_sector_outflow");
            let rate = t.fresh("edit_sector_rate");
            Some(single(
                format!("add the sector {level}"),
                vec![
                    aux(&rate, "0.1"),
                    flow(&inflow, "1"),
                    flow(&outflow, &format!("{level} * {rate}")),
                    stock(
                        &level,
                        "10",
                        std::slice::from_ref(&inflow),
                        std::slice::from_ref(&outflow),
                    ),
                ],
            ))
        }
        ScenarioKind::AddThenUndo => {
            let (f, n, ops) = t.add_parameter()?;
            let undo = vec![
                ModelOperation::DeleteVariable { ident: n.clone() },
                upsert(t.var(&f)?.clone()),
            ];
            Some(Scenario {
                kind,
                description: format!("add {n} to {f}, then take it back"),
                steps: vec![ops, undo],
                continuity: Vec::new(),
                returns_to_original: true,
            })
        }
    }
}

#[cfg(test)]
#[path = "edit_scenarios_tests.rs"]
mod tests;
