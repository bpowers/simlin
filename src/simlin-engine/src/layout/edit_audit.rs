// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The edit audit: what syncing a diagram after a model edit must and must not
//! do, checked from the edit's inputs and outputs alone.
//!
//! A diagram sync (`incremental_layout`, which MCP `edit_model`, libsimlin's
//! patch sync and pysimlin run after every edit) takes the view before the
//! edit, the patch, and the model after it. The audit states the contract a
//! sync keeps without reading how it keeps it:
//!
//! - **Scope.** An element the edit did not touch comes back exactly as it
//!   was. What counts as touched is derived from the two models and the patch:
//!   a deleted variable (its element, clouds, aliases and links go), a renamed
//!   one (only its name changes), one whose kind changed (it is rebuilt, and
//!   anything but a flow keeps its center), and a flow whose attachment changed
//!   (its pipe, valve and clouds may be rebuilt). A link whose dependency
//!   survives keeps its uid, endpoints and polarity, and its shape too unless
//!   an endpoint moved, when it keeps at least its kind (straight or curved).
//!   A link drawing no dependency the model has (an author's connector the
//!   extraction does not explain) survives unless the patch names its reader.
//!   A connector the view did not draw is drawn only where the edit is about
//!   it: into a variable the patch names, or between elements drawn for the
//!   first time, and a variable the view did not draw is drawn only when the
//!   patch names it. What an author left out elsewhere stays out.
//!   The one change allowed to an untouched element is wiring a flow endpoint
//!   the view left unattached to the flow's own cloud, which moves nothing.
//! - **Consistency.** The view after the edit agrees with the model after it:
//!   every variable drawn once with its kind, references resolve, links and
//!   drawn dependencies agree, flows attach where the stock lists say, and a
//!   flow the sync created or changed holds the strict flow invariants
//!   (`editing::invariants`). Only findings the edit introduced count: an
//!   imported view may carry inconsistencies of its own, and the edit is not
//!   charged for them.
//! - **Placement.** What the sync created or changed does not land on another
//!   element's shape, and a pipe it routed does not pass through a stock that
//!   is not one of its ends.
//!
//! The runner-level findings (a sync that failed, two syncs of one edit that
//! disagree, an edit that should have returned the original view) are raised
//! by `layout::edit_scenarios`, which drives edits through the production
//! patch and sync path and audits every step.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::common::canonicalize;
use crate::datamodel::view_element::{Flow, FlowPoint, LinkShape};
use crate::datamodel::{self, StockFlow, Variable, ViewElement};
use crate::diagram::common::Rect;
use crate::editing::invariants::{Mode, check_flow_invariants};
use crate::patch::{ModelOperation, ModelPatch};

use super::compute_dependency_metadata;
use super::config::LayoutConfig;
use super::metadata::ComputedMetadata;
use super::metrics::{MetricWeights, compute_layout_metrics, node_shape_box};

/// Coordinates within this distance are the same position.
const GEOMETRY_EPSILON: f64 = 1e-6;

/// Two shapes overlap when their boxes share more than this area (px^2): a
/// shared edge or a float's sliver is not an overlap anyone sees.
const MIN_OVERLAP_AREA: f64 = 1.0;

/// A pipe passes through a stock when it enters the stock's box shrunk by this
/// much on every side, so a pipe running along a face or ending on it does not.
const STOCK_INTERIOR_INSET: f64 = 0.5;

/// Which part of the contract a finding concerns.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Scope,
    Consistency,
    Placement,
    Runner,
}

/// One kind of finding.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// An element of a variable the edit deleted is still drawn: its own
    /// element, a cloud of its flow, or an alias of it.
    DeletedElementRemains,
    /// An element the edit did not touch was moved, rebuilt, removed, or
    /// otherwise changed.
    UntouchedElementChanged,
    /// A link whose dependency survived the edit was removed, re-created, or
    /// changed its endpoints, polarity or shape.
    UntouchedLinkChanged,
    /// A link whose dependency the edit removed (or one of whose ends it
    /// deleted), or a link drawing no dependency into a variable the patch
    /// names, is still drawn.
    StaleLinkRemains,
    /// A link the view did not draw was added for a dependency the edit is not
    /// about: the patch names neither its reader, nor was either end drawn for
    /// the first time. An author's view that leaves a connector out keeps it
    /// out.
    UnrelatedLinkAdded,
    /// A variable the view did not draw was drawn although the patch does not
    /// name it. An author's view that leaves a variable out keeps it out.
    UnrelatedElementAdded,
    /// A variable whose kind changed to anything but a flow was rebuilt away
    /// from where its old element was.
    RebuiltElementMoved,
    /// The view's own properties (viewport, zoom, name, font, polarity style)
    /// changed.
    ViewPropertiesChanged,
    /// A uid is duplicated or not positive.
    UidProblem,
    /// A variable has no element.
    VariableNotDrawn,
    /// A variable has more than one element.
    VariableDrawnTwice,
    /// An element's kind is not its variable's kind.
    ElementKindMismatch,
    /// A stock, flow, aux or module element names no variable.
    ElementNamesNoVariable,
    /// A link, cloud, alias or flow endpoint references an element that does
    /// not exist or cannot be referenced that way.
    DanglingReference,
    /// A link draws a dependency the model does not have.
    LinkWithoutDependency,
    /// A dependency between two drawn variables has no link.
    DependencyWithoutLink,
    /// A flow end is attached to something the stock lists do not say.
    FlowAttachmentMismatch,
    /// A flow the sync created or changed violates a strict flow invariant
    /// (or any flow violates a tolerant one).
    FlowInvariant,
    /// A created or changed element's shape covers another element's shape.
    ShapeOverlap,
    /// A created or changed pipe passes through a stock that is not one of
    /// its ends.
    PipeThroughStock,
    /// Two syncs of the same edit produced different views.
    NotDeterministic,
    /// An edit sequence expected to leave the view as it began did not.
    ReturnToOriginal,
    /// Applying the patch or syncing the view failed.
    SyncFailed,
}

impl FindingKind {
    pub const ALL: [FindingKind; 23] = [
        FindingKind::DeletedElementRemains,
        FindingKind::UntouchedElementChanged,
        FindingKind::UntouchedLinkChanged,
        FindingKind::StaleLinkRemains,
        FindingKind::UnrelatedLinkAdded,
        FindingKind::UnrelatedElementAdded,
        FindingKind::RebuiltElementMoved,
        FindingKind::ViewPropertiesChanged,
        FindingKind::UidProblem,
        FindingKind::VariableNotDrawn,
        FindingKind::VariableDrawnTwice,
        FindingKind::ElementKindMismatch,
        FindingKind::ElementNamesNoVariable,
        FindingKind::DanglingReference,
        FindingKind::LinkWithoutDependency,
        FindingKind::DependencyWithoutLink,
        FindingKind::FlowAttachmentMismatch,
        FindingKind::FlowInvariant,
        FindingKind::ShapeOverlap,
        FindingKind::PipeThroughStock,
        FindingKind::NotDeterministic,
        FindingKind::ReturnToOriginal,
        FindingKind::SyncFailed,
    ];

    /// A short stable name for reports.
    pub fn name(self) -> &'static str {
        match self {
            FindingKind::DeletedElementRemains => "deleted_element_remains",
            FindingKind::UntouchedElementChanged => "untouched_element_changed",
            FindingKind::UntouchedLinkChanged => "untouched_link_changed",
            FindingKind::StaleLinkRemains => "stale_link_remains",
            FindingKind::UnrelatedLinkAdded => "unrelated_link_added",
            FindingKind::UnrelatedElementAdded => "unrelated_element_added",
            FindingKind::RebuiltElementMoved => "rebuilt_element_moved",
            FindingKind::ViewPropertiesChanged => "view_properties_changed",
            FindingKind::UidProblem => "uid_problem",
            FindingKind::VariableNotDrawn => "variable_not_drawn",
            FindingKind::VariableDrawnTwice => "variable_drawn_twice",
            FindingKind::ElementKindMismatch => "element_kind_mismatch",
            FindingKind::ElementNamesNoVariable => "element_names_no_variable",
            FindingKind::DanglingReference => "dangling_reference",
            FindingKind::LinkWithoutDependency => "link_without_dependency",
            FindingKind::DependencyWithoutLink => "dependency_without_link",
            FindingKind::FlowAttachmentMismatch => "flow_attachment_mismatch",
            FindingKind::FlowInvariant => "flow_invariant",
            FindingKind::ShapeOverlap => "shape_overlap",
            FindingKind::PipeThroughStock => "pipe_through_stock",
            FindingKind::NotDeterministic => "not_deterministic",
            FindingKind::ReturnToOriginal => "return_to_original",
            FindingKind::SyncFailed => "sync_failed",
        }
    }

    pub fn layer(self) -> Layer {
        match self {
            FindingKind::DeletedElementRemains
            | FindingKind::UntouchedElementChanged
            | FindingKind::UntouchedLinkChanged
            | FindingKind::StaleLinkRemains
            | FindingKind::UnrelatedLinkAdded
            | FindingKind::UnrelatedElementAdded
            | FindingKind::RebuiltElementMoved
            | FindingKind::ViewPropertiesChanged => Layer::Scope,
            FindingKind::UidProblem
            | FindingKind::VariableNotDrawn
            | FindingKind::VariableDrawnTwice
            | FindingKind::ElementKindMismatch
            | FindingKind::ElementNamesNoVariable
            | FindingKind::DanglingReference
            | FindingKind::LinkWithoutDependency
            | FindingKind::DependencyWithoutLink
            | FindingKind::FlowAttachmentMismatch
            | FindingKind::FlowInvariant => Layer::Consistency,
            FindingKind::ShapeOverlap | FindingKind::PipeThroughStock => Layer::Placement,
            FindingKind::NotDeterministic
            | FindingKind::ReturnToOriginal
            | FindingKind::SyncFailed => Layer::Runner,
        }
    }
}

/// One thing a sync got wrong.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// What the finding is about, in the model's idents after the edit
    /// (`births`, `cloud of births`, `link birth_rate -> births`), so a finding
    /// before the edit and the same finding after it compare equal across a
    /// rename.
    pub subject: String,
    pub detail: String,
    /// Where it is on the view after the edit (`[left, top, right, bottom]`),
    /// when it is anywhere.
    pub region: Option<[f64; 4]>,
}

impl Finding {
    pub fn new(
        kind: FindingKind,
        subject: impl Into<String>,
        detail: impl Into<String>,
        region: Option<[f64; 4]>,
    ) -> Finding {
        Finding {
            kind,
            subject: subject.into(),
            detail: detail.into(),
            region,
        }
    }
}

/// How far an element the edit rebuilt moved.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct Displacement {
    pub subject: String,
    pub distance: f64,
}

/// The audit of one edit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, serde::Serialize)]
pub struct EditAudit {
    pub findings: Vec<Finding>,
    /// Elements the edit rebuilt (kind changes, re-attached flows), and how far
    /// each one's center or valve moved.
    pub displacements: Vec<Displacement>,
    /// The layout-quality cost of the view before and after the edit.
    pub cost_before: f64,
    pub cost_after: f64,
}

impl EditAudit {
    pub fn kinds(&self) -> BTreeSet<FindingKind> {
        self.findings.iter().map(|f| f.kind).collect()
    }
}

/// One edit: the model and view before it, the patch, and the model and view
/// after it.
pub struct EditInput<'a> {
    pub model_name: &'a str,
    pub before: &'a datamodel::Project,
    pub before_view: &'a StockFlow,
    pub patch: &'a ModelPatch,
    pub after: &'a datamodel::Project,
    pub after_view: &'a StockFlow,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Stock,
    Flow,
    Aux,
    Module,
}

fn var_kind(v: &Variable) -> VarKind {
    match v {
        Variable::Stock(_) => VarKind::Stock,
        Variable::Flow(_) => VarKind::Flow,
        Variable::Aux(_) => VarKind::Aux,
        Variable::Module(_) => VarKind::Module,
    }
}

fn element_kind(e: &ViewElement) -> Option<VarKind> {
    match e {
        ViewElement::Stock(_) => Some(VarKind::Stock),
        ViewElement::Flow(_) => Some(VarKind::Flow),
        ViewElement::Aux(_) => Some(VarKind::Aux),
        ViewElement::Module(_) => Some(VarKind::Module),
        _ => None,
    }
}

/// The canonical ident a stock, flow, aux or module element names.
fn named_ident(e: &ViewElement) -> Option<String> {
    element_kind(e)?;
    e.get_name().map(|n| canonicalize(n).into_owned())
}

fn center(e: &ViewElement) -> Option<(f64, f64)> {
    match e {
        ViewElement::Aux(a) => Some((a.x, a.y)),
        ViewElement::Stock(s) => Some((s.x, s.y)),
        ViewElement::Flow(f) => Some((f.x, f.y)),
        ViewElement::Module(m) => Some((m.x, m.y)),
        ViewElement::Alias(a) => Some((a.x, a.y)),
        ViewElement::Cloud(c) => Some((c.x, c.y)),
        ViewElement::Link(_) | ViewElement::Group(_) => None,
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn region_of(e: &ViewElement) -> Option<[f64; 4]> {
    let r = node_shape_box(e)?;
    Some([r.left, r.top, r.right, r.bottom])
}

/// The renames a patch applies, from each ident before the edit to its ident
/// after, in the order the patch applies them.
struct Renames(HashMap<String, String>);

impl Renames {
    fn of(patch: &ModelPatch) -> Renames {
        let mut map: HashMap<String, String> = HashMap::new();
        for op in &patch.ops {
            if let ModelOperation::RenameVariable { from, to } = op {
                let from = canonicalize(from).into_owned();
                let to = canonicalize(to).into_owned();
                let mut chained = false;
                for target in map.values_mut() {
                    if *target == from {
                        *target = to.clone();
                        chained = true;
                    }
                }
                if !chained {
                    map.insert(from, to);
                }
            }
        }
        Renames(map)
    }

    fn image(&self, ident: &str) -> String {
        self.0
            .get(ident)
            .cloned()
            .unwrap_or_else(|| ident.to_string())
    }

    /// The ident before the edit that `ident` after it had.
    fn preimage(&self, ident: &str) -> String {
        self.0
            .iter()
            .find(|(_, to)| to.as_str() == ident)
            .map(|(from, _)| from.clone())
            .unwrap_or_else(|| ident.to_string())
    }
}

/// One side of an edit: a model, the dependencies its diagram draws, and a
/// view.
struct Side<'a> {
    kinds: BTreeMap<String, VarKind>,
    meta: ComputedMetadata,
    /// `(dependency, dependent)` for every dependency a diagram draws: a
    /// variable's reads, less a stock's own inflows and outflows (a pipe draws
    /// those).
    edges: BTreeSet<(String, String)>,
    view: &'a StockFlow,
    by_uid: HashMap<i32, &'a ViewElement>,
}

impl<'a> Side<'a> {
    fn new(
        project: &datamodel::Project,
        model_name: &str,
        view: &'a StockFlow,
    ) -> Option<Side<'a>> {
        let model = project.get_model(model_name)?;
        let meta = compute_dependency_metadata(project, model_name, None)?;
        let kinds = model
            .variables
            .iter()
            .map(|v| (canonicalize(v.get_ident()).into_owned(), var_kind(v)))
            .collect();
        let mut edges = BTreeSet::new();
        for (var, deps) in &meta.dep_graph {
            let listed = |lists: &HashMap<String, Vec<String>>, dep: &str| {
                lists.get(var).is_some_and(|l| l.iter().any(|f| f == dep))
            };
            for dep in deps {
                if dep == var
                    || listed(&meta.stock_to_inflows, dep)
                    || listed(&meta.stock_to_outflows, dep)
                {
                    continue;
                }
                edges.insert((dep.clone(), var.clone()));
            }
        }
        let by_uid = view.elements.iter().map(|e| (e.get_uid(), e)).collect();
        Some(Side {
            kinds,
            meta,
            edges,
            view,
            by_uid,
        })
    }

    /// The named element a link endpoint draws, through an alias, with its
    /// ident.
    fn endpoint(&self, uid: i32) -> Option<(String, &'a ViewElement)> {
        let e = *self.by_uid.get(&uid)?;
        let e = match e {
            ViewElement::Alias(a) => *self.by_uid.get(&a.alias_of_uid)?,
            other => other,
        };
        named_ident(e).map(|i| (i, e))
    }

    fn first_named(&self, ident: &str) -> Option<&'a ViewElement> {
        self.view
            .elements
            .iter()
            .find(|e| named_ident(e).as_deref() == Some(ident))
    }

    /// The stock idents a flow element's two ends are attached to.
    fn attachment(&self, flow: &Flow) -> (Option<String>, Option<String>) {
        let end = |p: Option<&FlowPoint>| -> Option<String> {
            let uid = p?.attached_to_uid?;
            match self.by_uid.get(&uid)? {
                ViewElement::Stock(s) => Some(canonicalize(&s.name).into_owned()),
                _ => None,
            }
        };
        (end(flow.points.first()), end(flow.points.last()))
    }

    fn expected_attachment(&self, flow_ident: &str) -> (Option<String>, Option<String>) {
        let (from, to) = self.meta.connected_stocks(flow_ident);
        (from.map(str::to_string), to.map(str::to_string))
    }
}

fn subject_of(side: &Side, e: &ViewElement, name: &dyn Fn(&str) -> String) -> String {
    match e {
        ViewElement::Link(l) => {
            let end = |uid: i32| {
                side.endpoint(uid)
                    .map(|(i, _)| name(&i))
                    .unwrap_or_else(|| format!("#{uid}"))
            };
            format!("link {} -> {}", end(l.from_uid), end(l.to_uid))
        }
        ViewElement::Cloud(c) => match side.by_uid.get(&c.flow_uid).and_then(|f| named_ident(f)) {
            Some(f) => format!("cloud of {}", name(&f)),
            None => format!("cloud #{}", c.uid),
        },
        ViewElement::Alias(a) => match side.endpoint(a.alias_of_uid) {
            Some((i, _)) => format!("alias of {}", name(&i)),
            None => format!("alias #{}", a.uid),
        },
        ViewElement::Group(g) => format!("group {}", g.name),
        named => named_ident(named)
            .map(|i| name(&i))
            .unwrap_or_else(|| format!("#{}", named.get_uid())),
    }
}

fn set_name(e: &mut ViewElement, name: &str) {
    match e {
        ViewElement::Aux(a) => a.name = name.to_string(),
        ViewElement::Stock(s) => s.name = name.to_string(),
        ViewElement::Flow(f) => f.name = name.to_string(),
        ViewElement::Module(m) => m.name = name.to_string(),
        _ => {}
    }
}

/// `before` as an untouched element is allowed to come back, given what came
/// back: renamed when the new name is the rename's `image`, and with a flow
/// endpoint the view left unattached wired to the flow's own cloud.
fn allowed_form(
    before: &ViewElement,
    after: &ViewElement,
    after_side: &Side,
    image: &str,
) -> ViewElement {
    let mut expect = before.clone();
    if let Some(n) = after.get_name()
        && named_ident(after).as_deref() == Some(image)
    {
        set_name(&mut expect, n);
    }
    if let (ViewElement::Flow(f0), ViewElement::Flow(f1)) = (&mut expect, after)
        && f0.points.len() == f1.points.len()
        && !f0.points.is_empty()
    {
        let last = f0.points.len() - 1;
        for i in [0, last] {
            if f0.points[i].attached_to_uid.is_none()
                && let Some(u) = f1.points[i].attached_to_uid
                && matches!(after_side.by_uid.get(&u), Some(ViewElement::Cloud(c)) if c.flow_uid == f0.uid)
            {
                f0.points[i].attached_to_uid = Some(u);
            }
        }
    }
    expect
}

/// What differs between an element and what came back, for a finding's
/// detail.
fn difference(expected: &ViewElement, got: &ViewElement) -> String {
    match (expected, got) {
        (ViewElement::Link(a), ViewElement::Link(b)) => {
            if a.from_uid != b.from_uid || a.to_uid != b.to_uid {
                "endpoints changed".to_string()
            } else if a.polarity != b.polarity {
                "polarity changed".to_string()
            } else {
                format!("shape {} -> {}", shape_name(&a.shape), shape_name(&b.shape))
            }
        }
        (ViewElement::Flow(a), ViewElement::Flow(b)) => {
            if a.points != b.points {
                format!(
                    "pipe changed ({} -> {} points)",
                    a.points.len(),
                    b.points.len()
                )
            } else if (a.x, a.y) != (b.x, b.y) {
                format!("valve moved {:.1}", distance((a.x, a.y), (b.x, b.y)))
            } else {
                "label or name changed".to_string()
            }
        }
        _ => match (center(expected), center(got)) {
            (Some(a), Some(b)) if distance(a, b) > GEOMETRY_EPSILON => {
                format!("moved {:.1}", distance(a, b))
            }
            _ => "changed".to_string(),
        },
    }
}

fn shape_name(s: &LinkShape) -> String {
    match s {
        LinkShape::Straight => "straight".to_string(),
        LinkShape::Arc(a) => format!("arc({a:.1})"),
        LinkShape::MultiPoint(_) => "multipoint".to_string(),
    }
}

fn same_shape_kind(a: &LinkShape, b: &LinkShape) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Audit one edit.
pub fn audit_edit(input: &EditInput) -> EditAudit {
    let config = LayoutConfig::default();
    let weights = MetricWeights::default();
    let mut audit = EditAudit {
        cost_before: compute_layout_metrics(input.before_view, &config).weighted_cost(&weights),
        cost_after: compute_layout_metrics(input.after_view, &config).weighted_cost(&weights),
        ..EditAudit::default()
    };
    let (Some(before), Some(after)) = (
        Side::new(input.before, input.model_name, input.before_view),
        Side::new(input.after, input.model_name, input.after_view),
    ) else {
        audit.findings.push(Finding::new(
            FindingKind::SyncFailed,
            input.model_name,
            "the model is missing on one side of the edit",
            None,
        ));
        return audit;
    };
    let renames = Renames::of(input.patch);

    let changed = changed_uids(&before, &after, &renames);
    scope_findings(&before, &after, &renames, input.patch, &mut audit);

    let image = |i: &str| renames.image(i);
    let identity = |i: &str| i.to_string();
    let before_keys: HashSet<(FindingKind, String)> =
        consistency_findings(&before, &HashSet::new(), &image)
            .into_iter()
            .map(|f| (f.kind, f.subject))
            .collect();
    for finding in consistency_findings(&after, &changed, &identity) {
        if !before_keys.contains(&(finding.kind, finding.subject.clone())) {
            audit.findings.push(finding);
        }
    }
    placement_findings(&after, &changed, &mut audit.findings);
    audit
}

/// The uids of elements on the after view that are new, or differ from the
/// element of the same uid before the edit in anything but an allowed rename
/// or repair: what the sync created or changed.
fn changed_uids(before: &Side, after: &Side, renames: &Renames) -> HashSet<i32> {
    after
        .view
        .elements
        .iter()
        .filter(|e1| match before.by_uid.get(&e1.get_uid()) {
            None => true,
            Some(e0) => {
                let image = named_ident(e0)
                    .map(|i| renames.image(&i))
                    .unwrap_or_default();
                allowed_form(e0, e1, after, &image) != **e1
            }
        })
        .map(|e| e.get_uid())
        .collect()
}

/// A finding when `e0`, an element the edit did not touch, did not come back
/// as `allowed_form` allows.
fn unchanged_finding(
    e0: &ViewElement,
    e1: Option<&ViewElement>,
    after: &Side,
    image: &str,
    subject: &str,
) -> Option<Finding> {
    match e1 {
        None => Some(Finding::new(
            FindingKind::UntouchedElementChanged,
            subject,
            "removed",
            region_of(e0),
        )),
        Some(e1) => {
            let expect = allowed_form(e0, e1, after, image);
            (expect != *e1).then(|| {
                Finding::new(
                    FindingKind::UntouchedElementChanged,
                    subject,
                    difference(&expect, e1),
                    region_of(e1),
                )
            })
        }
    }
}

/// The scope layer: walk every element of the view before the edit and check
/// that what came back is what the edit allows.
fn scope_findings(
    before: &Side,
    after: &Side,
    renames: &Renames,
    patch: &ModelPatch,
    audit: &mut EditAudit,
) {
    let image = |i: &str| renames.image(i);
    // The variables the patch names, by their idents after it: the readers
    // whose connectors the edit is about.
    let named: HashSet<String> = patch.ops.iter().filter_map(named_by_op).collect();
    let deleted = |i0: &str| !after.kinds.contains_key(&renames.image(i0));
    let kind_changed = |i0: &str| {
        matches!(
            (before.kinds.get(i0), after.kinds.get(&renames.image(i0))),
            (Some(a), Some(b)) if a != b
        )
    };
    // A flow is re-attached when the stocks its drawn ends name (carried
    // through renames) are not the stocks the model after the edit lists it
    // on.
    let reattached = |i0: &str, flow: &Flow| {
        let (from, to) = before.attachment(flow);
        let drawn = (
            from.map(|s| renames.image(&s)),
            to.map(|s| renames.image(&s)),
        );
        drawn != after.expected_attachment(&renames.image(i0))
    };
    // Elements whose center differs across the edit: a link touching one may
    // re-bow.
    let moved: HashSet<i32> = before
        .view
        .elements
        .iter()
        .filter_map(|e0| {
            let c0 = center(e0)?;
            let c1 = after.by_uid.get(&e0.get_uid()).and_then(|e1| center(e1))?;
            (distance(c0, c1) > GEOMETRY_EPSILON).then_some(e0.get_uid())
        })
        .collect();

    let mut findings: Vec<Finding> = Vec::new();
    let mut displacements: Vec<Displacement> = Vec::new();
    for e0 in &before.view.elements {
        let uid = e0.get_uid();
        let e1 = after.by_uid.get(&uid).copied();
        let subject = subject_of(before, e0, &image);
        match e0 {
            ViewElement::Link(l) => {
                let (Some((from, _)), Some((to, _))) =
                    (before.endpoint(l.from_uid), before.endpoint(l.to_uid))
                else {
                    continue;
                };
                let edge = (renames.image(&from), renames.image(&to));
                // A link drawing no dependency is an author's choice the
                // extraction does not explain (a module port, an input the
                // dependency walk does not see, a deliberate annotation), so
                // only an edit to its reader may drop it. A dependency changes
                // only through its reader or a deleted end, so a link whose
                // dependency the edit removed always names a reader the patch
                // names.
                let survives = !deleted(&from)
                    && !deleted(&to)
                    && (after.edges.contains(&edge) || !named.contains(&edge.1));
                match (survives, e1) {
                    (false, Some(_)) => findings.push(Finding::new(
                        FindingKind::StaleLinkRemains,
                        subject,
                        "its dependency is gone",
                        None,
                    )),
                    (false, None) => {}
                    (true, None) => findings.push(Finding::new(
                        FindingKind::UntouchedLinkChanged,
                        subject,
                        "removed or re-created",
                        None,
                    )),
                    (true, Some(got @ ViewElement::Link(l1))) => {
                        let endpoint_moved = |uid: i32| {
                            moved.contains(&uid)
                                || before
                                    .endpoint(uid)
                                    .is_some_and(|(_, e)| moved.contains(&e.get_uid()))
                        };
                        let bow_may_change = endpoint_moved(l.from_uid) || endpoint_moved(l.to_uid);
                        let shape_ok = if bow_may_change {
                            same_shape_kind(&l.shape, &l1.shape)
                        } else {
                            l.shape == l1.shape
                        };
                        if l.from_uid != l1.from_uid
                            || l.to_uid != l1.to_uid
                            || l.polarity != l1.polarity
                            || !shape_ok
                        {
                            findings.push(Finding::new(
                                FindingKind::UntouchedLinkChanged,
                                subject,
                                difference(e0, got),
                                None,
                            ));
                        }
                    }
                    (true, Some(_)) => findings.push(Finding::new(
                        FindingKind::UntouchedLinkChanged,
                        subject,
                        "its uid now names another element",
                        None,
                    )),
                }
            }
            ViewElement::Cloud(c) => {
                let flow = before.by_uid.get(&c.flow_uid).copied();
                let Some((flow, f0)) = flow.and_then(|f| named_ident(f).map(|i| (f, i))) else {
                    findings.extend(unchanged_finding(e0, e1, after, "", &subject));
                    continue;
                };
                if deleted(&f0) {
                    if e1.is_some() {
                        findings.push(Finding::new(
                            FindingKind::DeletedElementRemains,
                            subject,
                            "its flow was deleted",
                            e1.and_then(region_of),
                        ));
                    }
                    continue;
                }
                let rebuilt =
                    kind_changed(&f0) || matches!(flow, ViewElement::Flow(f) if reattached(&f0, f));
                if !rebuilt {
                    findings.extend(unchanged_finding(e0, e1, after, "", &subject));
                }
            }
            ViewElement::Alias(a) => match before.endpoint(a.alias_of_uid) {
                Some((target, _)) if deleted(&target) => {
                    if e1.is_some() {
                        findings.push(Finding::new(
                            FindingKind::DeletedElementRemains,
                            subject,
                            "the variable was deleted",
                            e1.and_then(region_of),
                        ));
                    }
                }
                _ => findings.extend(unchanged_finding(e0, e1, after, "", &subject)),
            },
            ViewElement::Group(_) => {
                findings.extend(unchanged_finding(e0, e1, after, "", &subject));
            }
            named => {
                let Some(i0) = named_ident(named) else {
                    continue;
                };
                if !before.kinds.contains_key(&i0) {
                    // An element naming no variable: no edit touches it.
                    findings.extend(unchanged_finding(e0, e1, after, &i0, &subject));
                    continue;
                }
                let i1 = renames.image(&i0);
                if deleted(&i0) {
                    if e1.is_some_and(|e| named_ident(e).is_some()) {
                        findings.push(Finding::new(
                            FindingKind::DeletedElementRemains,
                            subject,
                            "the variable was deleted",
                            e1.and_then(region_of),
                        ));
                    }
                    continue;
                }
                if kind_changed(&i0) {
                    let rebuilt = after.first_named(&i1);
                    if let (Some(c0), Some(c1)) = (center(named), rebuilt.and_then(center)) {
                        let d = distance(c0, c1);
                        displacements.push(Displacement {
                            subject: i1.clone(),
                            distance: d,
                        });
                        if after.kinds.get(&i1) != Some(&VarKind::Flow) && d > GEOMETRY_EPSILON {
                            findings.push(Finding::new(
                                FindingKind::RebuiltElementMoved,
                                subject,
                                format!("moved {d:.1}"),
                                rebuilt.and_then(region_of),
                            ));
                        }
                    }
                    continue;
                }
                if let ViewElement::Flow(f) = named
                    && reattached(&i0, f)
                {
                    if let (Some(c0), Some(c1)) =
                        (center(named), after.first_named(&i1).and_then(center))
                    {
                        displacements.push(Displacement {
                            subject: i1.clone(),
                            distance: distance(c0, c1),
                        });
                    }
                    continue;
                }
                findings.extend(unchanged_finding(e0, e1, after, &i1, &subject));
            }
        }
    }

    let (v0, v1) = (before.view, after.view);
    if v0.name != v1.name
        || v0.view_box != v1.view_box
        || v0.zoom != v1.zoom
        || v0.use_lettered_polarity != v1.use_lettered_polarity
        || v0.font != v1.font
    {
        findings.push(Finding::new(
            FindingKind::ViewPropertiesChanged,
            "view",
            "name, viewport, zoom, font or polarity style changed",
            None,
        ));
    }
    audit.findings.extend(findings);
    audit.displacements.extend(displacements);

    // A connector the view did not draw may be drawn into a variable the patch
    // names, or between elements drawn for the first time, which carry no
    // author's choice about their connectors; a variable the view did not draw
    // may be drawn only when the patch names it. Anywhere else it is a change
    // to a part of the diagram the edit is not about.
    let untouched_var = |i1: &str| {
        let i0 = renames.preimage(i1);
        before.kinds.contains_key(&i0) && !kind_changed(&i0)
    };
    let before_link_edges: HashSet<(String, String)> = before
        .view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l) => Some((
                renames.image(&before.endpoint(l.from_uid)?.0),
                renames.image(&before.endpoint(l.to_uid)?.0),
            )),
            _ => None,
        })
        .collect();
    let before_drawn: HashSet<String> = before
        .view
        .elements
        .iter()
        .filter_map(|e| named_ident(e).map(|i| renames.image(&i)))
        .collect();
    for e1 in &after.view.elements {
        if before.by_uid.contains_key(&e1.get_uid()) {
            continue;
        }
        if let ViewElement::Link(l) = e1 {
            if let (Some((from, _)), Some((to, _))) =
                (after.endpoint(l.from_uid), after.endpoint(l.to_uid))
            {
                let related = named.contains(&to)
                    || !before_drawn.contains(&from)
                    || !before_drawn.contains(&to);
                if !related && !before_link_edges.contains(&(from.clone(), to.clone())) {
                    audit.findings.push(Finding::new(
                        FindingKind::UnrelatedLinkAdded,
                        format!("link {from} -> {to}"),
                        "the patch names neither its reader nor a newly drawn end",
                        None,
                    ));
                }
            }
        } else if let Some(i1) = named_ident(e1)
            && untouched_var(&i1)
            && !before_drawn.contains(&i1)
            && !named.contains(&i1)
        {
            audit.findings.push(Finding::new(
                FindingKind::UnrelatedElementAdded,
                i1,
                "the view did not draw it and the patch does not name it",
                region_of(e1),
            ));
        }
    }
}

/// The variable an operation defines, by its ident after the patch: the reader
/// whose connectors the operation is about.
fn named_by_op(op: &ModelOperation) -> Option<String> {
    let ident = match op {
        ModelOperation::UpsertStock(s) => &s.ident,
        ModelOperation::UpsertFlow(f) => &f.ident,
        ModelOperation::UpsertAux(a) => &a.ident,
        ModelOperation::UpsertModule(m) => &m.ident,
        ModelOperation::RenameVariable { to, .. } => to,
        ModelOperation::UpdateStockFlows { ident, .. } => ident,
        ModelOperation::DeleteVariable { .. }
        | ModelOperation::UpsertView { .. }
        | ModelOperation::DeleteView { .. }
        | ModelOperation::SetLoopName { .. }
        | ModelOperation::EditView { .. } => return None,
    };
    Some(canonicalize(ident).into_owned())
}

/// The consistency layer over one side. `routed` holds the uids the sync
/// created or changed: their flows are held to the strict flow invariants.
/// `name` maps an ident on this side to the after-edit ident findings are
/// keyed by.
fn consistency_findings(
    side: &Side,
    routed: &HashSet<i32>,
    name: &dyn Fn(&str) -> String,
) -> Vec<Finding> {
    let mut out = Vec::new();

    let mut uid_counts: BTreeMap<i32, usize> = BTreeMap::new();
    for e in &side.view.elements {
        *uid_counts.entry(e.get_uid()).or_default() += 1;
    }
    for (uid, count) in &uid_counts {
        if *count > 1 || *uid <= 0 {
            out.push(Finding::new(
                FindingKind::UidProblem,
                format!("uid {uid}"),
                if *count > 1 {
                    format!("{count} elements share it")
                } else {
                    "not positive".to_string()
                },
                None,
            ));
        }
    }

    let mut drawn: BTreeMap<String, Vec<&ViewElement>> = BTreeMap::new();
    for e in &side.view.elements {
        if let Some(i) = named_ident(e) {
            drawn.entry(i).or_default().push(e);
        }
    }
    for (ident, kind) in &side.kinds {
        match drawn.get(ident) {
            None => out.push(Finding::new(
                FindingKind::VariableNotDrawn,
                name(ident),
                "",
                None,
            )),
            Some(elements) => {
                if elements.len() > 1 {
                    out.push(Finding::new(
                        FindingKind::VariableDrawnTwice,
                        name(ident),
                        format!("{} elements", elements.len()),
                        region_of(elements[1]),
                    ));
                }
                if elements.iter().any(|e| element_kind(e) != Some(*kind)) {
                    out.push(Finding::new(
                        FindingKind::ElementKindMismatch,
                        name(ident),
                        "",
                        region_of(elements[0]),
                    ));
                }
            }
        }
    }
    for (ident, elements) in &drawn {
        if !side.kinds.contains_key(ident) {
            out.push(Finding::new(
                FindingKind::ElementNamesNoVariable,
                name(ident),
                "",
                region_of(elements[0]),
            ));
        }
    }

    let mut link_edges: HashSet<(String, String)> = HashSet::new();
    for e in &side.view.elements {
        match e {
            ViewElement::Link(l) => match (side.endpoint(l.from_uid), side.endpoint(l.to_uid)) {
                (Some((from, _)), Some((to, _))) => {
                    let edge = (from, to);
                    if !side.edges.contains(&edge) {
                        out.push(Finding::new(
                            FindingKind::LinkWithoutDependency,
                            format!("link {} -> {}", name(&edge.0), name(&edge.1)),
                            "",
                            None,
                        ));
                    }
                    link_edges.insert(edge);
                }
                _ => out.push(Finding::new(
                    FindingKind::DanglingReference,
                    format!("link #{}", l.uid),
                    "an end is not a named element or an alias of one",
                    None,
                )),
            },
            ViewElement::Alias(a) => {
                if side.endpoint(a.alias_of_uid).is_none() {
                    out.push(Finding::new(
                        FindingKind::DanglingReference,
                        format!("alias #{}", a.uid),
                        "aliases no named element",
                        region_of(e),
                    ));
                }
            }
            ViewElement::Cloud(c) => {
                let owner = match side.by_uid.get(&c.flow_uid) {
                    Some(ViewElement::Flow(f)) => Some(f),
                    _ => None,
                };
                let ends = owner.map_or(0, |f| {
                    let last = f.points.len().saturating_sub(1);
                    f.points
                        .iter()
                        .enumerate()
                        .filter(|(i, p)| {
                            (*i == 0 || *i == last) && p.attached_to_uid == Some(c.uid)
                        })
                        .count()
                });
                if ends != 1 {
                    out.push(Finding::new(
                        FindingKind::DanglingReference,
                        subject_of(side, e, name),
                        match owner {
                            None => "its flow does not exist".to_string(),
                            Some(_) => format!("an endpoint of its flow {ends} times"),
                        },
                        region_of(e),
                    ));
                }
            }
            ViewElement::Flow(f) => {
                for p in &f.points {
                    if let Some(uid) = p.attached_to_uid
                        && !matches!(
                            side.by_uid.get(&uid),
                            Some(ViewElement::Stock(_)) | Some(ViewElement::Cloud(_))
                        )
                    {
                        out.push(Finding::new(
                            FindingKind::DanglingReference,
                            subject_of(side, e, name),
                            format!("attached to #{uid}, which is no stock or cloud"),
                            region_of(e),
                        ));
                    }
                }
                if let Some(ident) = named_ident(e)
                    && side.kinds.get(&ident) == Some(&VarKind::Flow)
                {
                    let (from, to) = side.expected_attachment(&ident);
                    let end_ok = |p: Option<&FlowPoint>, expected: &Option<String>| {
                        let attached = p
                            .and_then(|p| p.attached_to_uid)
                            .and_then(|u| side.by_uid.get(&u));
                        match (expected, attached) {
                            (Some(stock), Some(ViewElement::Stock(s))) => {
                                canonicalize(&s.name) == stock.as_str()
                            }
                            (Some(_), _) => false,
                            (None, None) => true,
                            (None, Some(ViewElement::Cloud(c))) => c.flow_uid == f.uid,
                            (None, Some(_)) => false,
                        }
                    };
                    if !end_ok(f.points.first(), &from) || !end_ok(f.points.last(), &to) {
                        out.push(Finding::new(
                            FindingKind::FlowAttachmentMismatch,
                            name(&ident),
                            format!(
                                "the model lists it from {} to {}",
                                from.as_deref().unwrap_or("a cloud"),
                                to.as_deref().unwrap_or("a cloud")
                            ),
                            region_of(e),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    for (dep, var) in &side.edges {
        if drawn.contains_key(dep)
            && drawn.contains_key(var)
            && !link_edges.contains(&(dep.clone(), var.clone()))
        {
            out.push(Finding::new(
                FindingKind::DependencyWithoutLink,
                format!("link {} -> {}", name(dep), name(var)),
                "",
                None,
            ));
        }
    }

    for v in check_flow_invariants(
        &side.view.elements,
        Mode::Strict {
            routed: Some(routed),
        },
    ) {
        let flow = side.by_uid.get(&v.uid).copied();
        let subject = match flow.and_then(named_ident) {
            Some(i) => format!("{} {}", v.arm.name(), name(&i)),
            None => format!("{} #{}", v.arm.name(), v.uid),
        };
        out.push(Finding::new(
            FindingKind::FlowInvariant,
            subject,
            v.message.clone(),
            flow.and_then(region_of),
        ));
    }
    out
}

/// What happened to one element across an edit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Changed,
    Removed,
}

/// One element that differs across an edit, with where it is drawn (on the
/// view after the edit, or before it for a removed element).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct ViewChange {
    pub uid: i32,
    pub kind: ChangeKind,
    pub region: Option<[f64; 4]>,
}

/// Where an element is drawn: its shape, and for a flow its whole pipe.
fn drawn_region(e: &ViewElement) -> Option<[f64; 4]> {
    let mut region = region_of(e)?;
    if let ViewElement::Flow(f) = e {
        for p in &f.points {
            region = [
                region[0].min(p.x - 4.0),
                region[1].min(p.y - 4.0),
                region[2].max(p.x + 4.0),
                region[3].max(p.y + 4.0),
            ];
        }
    }
    Some(region)
}

/// Every element created, removed, or holding a different value across an
/// edit, by uid: the raw difference a reviewer draws over the renders, renames
/// and repairs included. Links have no region.
pub fn view_changes(before: &StockFlow, after: &StockFlow) -> Vec<ViewChange> {
    let old: HashMap<i32, &ViewElement> =
        before.elements.iter().map(|e| (e.get_uid(), e)).collect();
    let new: HashSet<i32> = after.elements.iter().map(ViewElement::get_uid).collect();
    let mut out: Vec<ViewChange> = after
        .elements
        .iter()
        .filter_map(|e| {
            let kind = match old.get(&e.get_uid()) {
                None => ChangeKind::Created,
                Some(o) if *o != e => ChangeKind::Changed,
                _ => return None,
            };
            Some(ViewChange {
                uid: e.get_uid(),
                kind,
                region: drawn_region(e),
            })
        })
        .collect();
    out.extend(
        before
            .elements
            .iter()
            .filter(|e| !new.contains(&e.get_uid()))
            .map(|e| ViewChange {
                uid: e.get_uid(),
                kind: ChangeKind::Removed,
                region: drawn_region(e),
            }),
    );
    out
}

fn overlap_area(a: &Rect, b: &Rect) -> f64 {
    let w = a.right.min(b.right) - a.left.max(b.left);
    let h = a.bottom.min(b.bottom) - a.top.max(b.top);
    w.max(0.0) * h.max(0.0)
}

/// Whether segment `a`-`b` enters the open box `r` (Liang-Barsky clipping):
/// a segment along an edge or ending on it does not.
fn segment_enters(a: (f64, f64), b: (f64, f64), r: &Rect) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - r.left),
        (dx, r.right - a.0),
        (-dy, a.1 - r.top),
        (dy, r.bottom - a.1),
    ] {
        if p.abs() < 1e-12 {
            if q <= 0.0 {
                return false;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t >= t1 {
                return false;
            }
            t0 = t0.max(t);
        } else {
            if t <= t0 {
                return false;
            }
            t1 = t1.min(t);
        }
    }
    t1 - t0 > 1e-9
}

/// The placement layer, over the elements the sync created or changed.
fn placement_findings(side: &Side, changed: &HashSet<i32>, out: &mut Vec<Finding>) {
    let name = |i: &str| i.to_string();
    let shapes: Vec<(&ViewElement, Rect)> = side
        .view
        .elements
        .iter()
        .filter_map(|e| node_shape_box(e).map(|r| (e, r)))
        .collect();
    let mut reported: HashSet<(i32, i32)> = HashSet::new();
    for (e, r) in &shapes {
        if !changed.contains(&e.get_uid()) {
            continue;
        }
        for (o, ro) in &shapes {
            if o.get_uid() == e.get_uid() {
                continue;
            }
            let pair = (e.get_uid().min(o.get_uid()), e.get_uid().max(o.get_uid()));
            let area = overlap_area(r, ro);
            if area > MIN_OVERLAP_AREA && reported.insert(pair) {
                out.push(Finding::new(
                    FindingKind::ShapeOverlap,
                    format!(
                        "{} over {}",
                        subject_of(side, e, &name),
                        subject_of(side, o, &name)
                    ),
                    format!("{area:.0} px^2"),
                    Some([
                        r.left.max(ro.left),
                        r.top.max(ro.top),
                        r.right.min(ro.right),
                        r.bottom.min(ro.bottom),
                    ]),
                ));
            }
        }
    }

    for e in &side.view.elements {
        let ViewElement::Flow(f) = e else { continue };
        if !changed.contains(&f.uid) {
            continue;
        }
        let terminals: HashSet<i32> = [f.points.first(), f.points.last()]
            .into_iter()
            .flatten()
            .filter_map(|p| p.attached_to_uid)
            .collect();
        for (o, ro) in &shapes {
            if !matches!(o, ViewElement::Stock(_)) || terminals.contains(&o.get_uid()) {
                continue;
            }
            let inner = Rect {
                left: ro.left + STOCK_INTERIOR_INSET,
                top: ro.top + STOCK_INTERIOR_INSET,
                right: ro.right - STOCK_INTERIOR_INSET,
                bottom: ro.bottom - STOCK_INTERIOR_INSET,
            };
            if f.points
                .windows(2)
                .any(|w| segment_enters((w[0].x, w[0].y), (w[1].x, w[1].y), &inner))
            {
                out.push(Finding::new(
                    FindingKind::PipeThroughStock,
                    format!(
                        "{} through {}",
                        subject_of(side, e, &name),
                        subject_of(side, o, &name)
                    ),
                    "",
                    Some([ro.left, ro.top, ro.right, ro.bottom]),
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "edit_audit_tests.rs"]
mod tests;
