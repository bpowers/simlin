// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `analyze_loops`: a run's feedback loops and which of them drove it, from
//! Loops That Matter.
//!
//! A run is made without the LTM overlay; its loops are analyzed when asked
//! for, by replaying its plan under the overlay in discovery mode (every
//! causal edge scored) and running discovery over the replay's results
//! ([`crate::analysis::discover_run_loops`]). The analysis is kept with the
//! run.
//!
//! The answer is bounded. Loops compete only within a cycle partition (the
//! stocks feedback connects), so it is given partition by partition: the
//! partition's stocks; a dominance timeline, the run cut where the lead
//! changes ([`crate::ltm_dominance::leader_timeline`]), each span naming the
//! loop that led it and its rivals, with their shares of the partition's loop
//! activity; and the partition's loops, every one the timeline names and the
//! most important others. A loop is reported with its session id, its polarity
//! as the run shows it, its chain from a stock around to the start with each
//! link's sign (a builtin's or macro's internal nodes left out, the links
//! across them composed and marked with the builtin), and its mean share. The
//! answer keeps to the outline's budget by listing fewer: the partitions and
//! their leaders first, then the other loops ([`Selection`]).
//!
//! Loop ids (`L1`, `L2`, ...) are the session's, keyed by the loop's cycle --
//! its node sequence as the engine has it, each element of an arrayed variable
//! and each builtin's instance a node, rotation-invariant and
//! direction-preserving -- so a loop keeps its id across runs and edits for
//! as long as it exists, from a run and from structure alike.
//!
//! A link's sign is the one the run gives it when the run scored it
//! ([`LinkPolarity::from_runtime_scores`]), and its equation's sign otherwise.
//!
//! A run in which no loop is active -- a model at rest, whose loop scores are
//! zero throughout, or whose stocks move by nothing but the rounding of what
//! they are computed from, which makes scores of noise -- has no dominance
//! and no runtime polarity to report. The answer says so, lists the model's loops from its structure with their
//! equations' signs, and names the repair: an experiment that disturbs the
//! model. A run in which some loops are active lists the model's other loops,
//! from its structure, as inactive.
//!
//! A run whose plan replaces equations reports what they cut: the links each
//! replaced variable read and no longer reads, and the model's loops through
//! them (its structural loops when the structure alone enumerates them, else
//! the loops of its current run).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, Variable};
use crate::db::{
    LtmMode, LtmOverlay, SimlinDb, SourceModel, SourceProject, set_project_ltm_discovery_mode,
};
use crate::ltm::{
    LinkPolarity, LoopPolarity, canonical_rotation, is_synthetic_node_name, strip_subscript,
};
use crate::ltm_dominance::{LEAD_TIE, MeanShares, leader_by_step, leader_timeline, strongest};
use crate::results::Results;

use super::evidence::Evidence;
use super::runs::{self, CURRENT, Run, RunPlan, RunStore};
use super::series::round;
use super::variables::LinkPolarityName;
use super::{Session, ToolError, Workspace, is_zero, names, resolve_model};

/// How long discovery may search a run for loops before it reports what it
/// found.
const DISCOVERY_BUDGET: Duration = Duration::from_secs(20);

/// The most spans a dominance timeline has.
pub(crate) const MAX_SPANS: usize = 12;

/// The share of a partition's loop activity the strongest loop holds at a
/// step for it to lead there: discovery's own floor for a loop worth
/// reporting.
pub(crate) const ACTIVE_SHARE: f64 = 0.001;

/// The share of the leader's a loop holds over a span to be its rival there.
pub(crate) const RIVAL_SHARE: f64 = 0.5;

/// The most loops a timeline span names: its leader and its rivals.
pub(crate) const MAX_LEADERS: usize = 3;

/// The share below which a partition's leader does not dominate it: its
/// activity is spread across many loops.
pub(crate) const DOMINANT_SHARE: f64 = 0.1;

/// The loops a partition lists beyond those its timeline names, and the most
/// a cut or the inactive loops list.
pub(crate) const MAX_LOOPS: usize = 8;

/// The longest loop an overview gives the chain of; a longer loop is given by
/// its stocks, and whole when asked for by id.
pub(crate) const MAX_CHAIN: usize = 12;

/// The most loops one call asks for by id.
pub(crate) const MAX_LOOPS_BY_ID: usize = 4;

/// The most partitions an answer lists, largest first.
pub(crate) const MAX_PARTITIONS: usize = 6;

/// The most stocks a partition names.
pub(crate) const MAX_STOCKS: usize = 12;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AnalyzeLoopsInput {
    /// The run to analyze: "current" for the model as it is (when absent), or
    /// a run an experiment made.
    #[serde(default)]
    pub run: Option<String>,
    /// Only the loops through this variable (any of its elements, or the one
    /// named, as in `population[north]`), and the partitions they are in.
    #[serde(default)]
    pub through: Option<String>,
    /// Loops to report whole, by the ids earlier answers gave them: each with
    /// its full chain, in its partition, and no dominance timeline. At most
    /// 4, and not with `through`.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 4)))]
    pub loops: Vec<String>,
}

/// Where an answer's loops come from.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LoopBasis {
    /// The run's loop scores: loops active in the run, with their dominance
    /// and the polarity the run shows.
    Run,
    /// The model's structure, because no loop was active in the run: loops
    /// with their equations' polarity, and no dominance.
    Structure,
}

/// A loop's polarity: reinforcing (it amplifies change) or balancing (it
/// counteracts change). From a run, a loop that showed both is "mostly" the
/// one it showed at least 99% of the time, weighted by its score, and
/// undetermined otherwise; from structure, a loop with a link whose sign the
/// engine cannot tell is undetermined.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LoopPolarityName {
    Reinforcing,
    Balancing,
    MostlyReinforcing,
    MostlyBalancing,
    Undetermined,
}

impl LoopPolarityName {
    pub const ALL: [LoopPolarityName; 5] = [
        LoopPolarityName::Reinforcing,
        LoopPolarityName::Balancing,
        LoopPolarityName::MostlyReinforcing,
        LoopPolarityName::MostlyBalancing,
        LoopPolarityName::Undetermined,
    ];
}

impl From<LoopPolarity> for LoopPolarityName {
    fn from(polarity: LoopPolarity) -> LoopPolarityName {
        match polarity {
            LoopPolarity::Reinforcing => LoopPolarityName::Reinforcing,
            LoopPolarity::Balancing => LoopPolarityName::Balancing,
            LoopPolarity::MostlyReinforcing => LoopPolarityName::MostlyReinforcing,
            LoopPolarity::MostlyBalancing => LoopPolarityName::MostlyBalancing,
            LoopPolarity::Undetermined => LoopPolarityName::Undetermined,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeLoopsOutput {
    pub revision: u64,
    pub run: String,
    pub basis: LoopBasis,
    /// The variable `through` named, as the model names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    /// The loops the analysis has (those through `through`, when given): the
    /// loops active in the run, or, from structure, the model's.
    pub found: usize,
    /// Whether those are every such loop. False when the analysis left out
    /// loops that never held a thousandth of their partition's activity;
    /// when it kept only the most important of the run's loops; when the
    /// model has too many to enumerate, so the run's were found by searching
    /// for the strongest (a sample); when the model has too many for its
    /// structure alone to list; or when it has a conveyor or a queue, whose
    /// loops are not analyzed.
    /// That no loop goes through a variable is shown only from every loop
    /// of the model: a complete analysis, or the active loops with the
    /// `inactive` ones, where the structure lists them.
    pub complete: bool,
    /// The partitions the loops are in, largest first.
    pub partitions: Vec<PartitionReport>,
    /// The model's loops, from its structure, that are not among this run's
    /// (those through `through`, when given): inactive in it, or among those
    /// the analysis left out. Absent when the run has every loop of the
    /// model, from structure, and when the model has too many loops for its
    /// structure alone to list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inactive: Option<InactiveLoops>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<OmittedLoops>,
    /// Loops asked for by id that the run's analysis does not have, active,
    /// inactive or cut: a loop of another revision of the model. A loop
    /// asked for that is inactive in the run or cut from it is listed whole
    /// under `inactive` or `cut`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
    /// Loops asked for by id left out to keep the answer within its budget:
    /// ask for them in another call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<String>,
    /// What the run's replaced equations cut.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut: Option<CutReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A cycle partition: stocks connected by feedback, and the loops among them,
/// which compete for dominance with each other and no others.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PartitionReport {
    /// Its stocks, as the model names them (an arrayed stock by element); at
    /// most 12. Empty for the loops that pass through no stock of this model
    /// (inside a module, or closed through a lagged value): each of those
    /// stands alone, so they have no timeline and no shares.
    pub stocks: Vec<String>,
    /// How many more stocks it has than it names.
    #[serde(skip_serializing_if = "is_zero")]
    pub other_stocks: usize,
    /// How many of its loops the analysis has (through `through`, when
    /// given).
    pub loop_count: usize,
    /// Which loops led, span by span: the run's saved steps from its start to
    /// its end, cut at the steps the lead changed, in at most 12 spans (the
    /// shortest joined to their neighbours past that), each from the time of
    /// its first step to the time of its last. With `through`, which of the
    /// loops through it led. Absent for loops from structure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dominance: Vec<DominanceSpan>,
    /// Every loop `dominance` names, then the most important others (the
    /// shortest, from structure); at most 8 besides those it names.
    pub loops: Vec<LoopReport>,
}

/// A span of a run and the loops that led it: first the loop with the
/// largest share over the span, then its rivals, those holding at least half
/// its share there, strongest first; at most 3 in all. None led a span in
/// which no loop (through `through`, when given) was the strongest. A span
/// names the single strongest loop, so where several loops take over
/// together (SIR's balancing loops after the epidemic's peak) the span
/// changes a few steps after the turn, when one of them alone passes the
/// loop that led before.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DominanceSpan {
    pub from: f64,
    pub to: f64,
    pub leaders: Vec<LoopShare>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LoopShare {
    /// The loop's id.
    #[serde(rename = "loop")]
    pub id: String,
    /// Its share of the partition's loop activity over the span, from 0 to 1:
    /// at each time the shares of all the partition's loops sum to 1, so
    /// where many loops are active each holds little.
    pub share: f64,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LoopReport {
    /// The session's id for this loop (`L1`, ...), the same in every run and
    /// revision while the loop exists.
    pub id: String,
    /// The name the model gives the loop, if it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub polarity: LoopPolarityName,
    /// How many variables the loop passes through.
    pub length: usize,
    /// The loop from a stock (when it has one) around to where it started:
    /// each step is a variable and the sign of the link from it to the next
    /// step's variable, the last step's link returning to the first. Absent
    /// for a loop of more than 12 variables, unless it was asked for by id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<Vec<ChainLink>>,
    /// The stocks the loop passes through, in its order: given for a loop
    /// whose chain is not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stocks: Vec<String>,
    /// Its share of the partition's loop activity, from 0 to 1, averaged over
    /// the steps at which the partition was active: the shares of a
    /// partition's loops sum to 1. Absent for loops from structure, and for a
    /// loop in no partition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share: Option<f64>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChainLink {
    /// The variable, as the model names it; an element of an arrayed one is
    /// subscripted.
    pub variable: String,
    /// The sign of the link from this variable to the next: the run's, when
    /// the run scored the link, so `?` there means its sign changed over the
    /// run; otherwise its equation's.
    pub polarity: LinkPolarityName,
    /// The builtins or macros the link passes through in the next variable's
    /// equation (`smth1`, `delay3`), when it does: such a link carries a
    /// delay, and is another link than a direct one between the same two
    /// variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// Loops the analysis has and the answer does not list.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct OmittedLoops {
    /// Partitions not listed.
    pub partitions: usize,
    /// Loops not listed, in listed partitions and the others. `through` finds
    /// the loops through a variable.
    pub loops: usize,
}

/// The model's loops that are not among a run's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct InactiveLoops {
    /// The shortest of them, with their equations' signs; at most 8.
    pub loops: Vec<LoopReport>,
    #[serde(skip_serializing_if = "is_zero")]
    pub other_loops: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CutReport {
    /// The links the replaced equations removed: what each replaced variable
    /// read before and no longer reads.
    pub links: Vec<CutLink>,
    /// The model's loops through them, which this run does not have; at most
    /// 8.
    pub loops: Vec<LoopReport>,
    #[serde(skip_serializing_if = "is_zero")]
    pub other_loops: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CutLink {
    pub from: String,
    pub to: String,
}

/// A run's loops as an analysis keeps them, before the session names them.
pub(crate) struct LoopAnalysis {
    basis: LoopBasis,
    times: Vec<f64>,
    loops: Vec<AnalyzedLoop>,
    /// Each partition's stocks (canonical, element-level), indexed by
    /// [`AnalyzedLoop::partition`].
    partitions: Vec<Vec<String>>,
    /// Whether discovery found the run's loops by searching for the strongest
    /// rather than enumerating them: a sample.
    sampled: bool,
    /// How many loops discovery kept before its cap, when the cap bound.
    capped_from: Option<usize>,
    /// How many of the run's loops discovery left out for never holding a
    /// thousandth of their partition's activity (its retention floor).
    negligible: usize,
    /// Whether the model's structure alone enumerates its loops (from
    /// structure, whether `loops` is all of them).
    enumerated: bool,
    /// Whether the model has a conveyor or a queue: a run of it scores no
    /// loop, and its structure leaves out the links they make.
    conveyors: bool,
    /// The model's loops from structure that the run does not have, shortest
    /// first; `None` from structure, and when the structure alone does not
    /// enumerate the model's loops.
    inactive: Option<Vec<AnalyzedLoop>>,
    cut: Option<Cut>,
}

impl LoopAnalysis {
    /// About how many bytes the analysis holds: its times, and each loop's
    /// score series and names, the inactive and cut loops included. What a
    /// run store counts a kept analysis as.
    pub(crate) fn bytes(&self) -> usize {
        let text = |names: &[String]| names.iter().map(String::len).sum::<usize>();
        let of_loop = |l: &AnalyzedLoop| {
            l.rel.len() * std::mem::size_of::<f64>()
                + text(&l.key)
                + l.chain
                    .iter()
                    .map(|step| step.node.len() + text(&step.via))
                    .sum::<usize>()
        };
        let cut = self.cut.iter().flat_map(|cut| &cut.loops);
        self.times.len() * std::mem::size_of::<f64>()
            + self
                .loops
                .iter()
                .chain(self.inactive.iter().flatten())
                .chain(cut)
                .map(of_loop)
                .sum::<usize>()
            + self.partitions.iter().map(|p| text(p)).sum::<usize>()
    }

    /// Whether `loops` is every loop of the run, or, from structure, of the
    /// model: what an absence can be shown from.
    fn complete(&self) -> bool {
        match self.basis {
            LoopBasis::Run => !self.sampled && self.capped_from.is_none() && self.negligible == 0,
            LoopBasis::Structure => self.enumerated && !self.conveyors,
        }
    }

    /// Whether the analysis knows every loop of the model, active in the run
    /// or not, so that an absence can be shown: it is complete, or the
    /// structure lists the model's loops, the ones the run left inactive
    /// among them.
    fn shows_absence(&self) -> bool {
        self.complete() || self.inactive.is_some()
    }

    /// The loops a loop competes with for dominance, itself among them: its
    /// partition's, or itself alone when it is in none.
    fn peers<'a>(&'a self, l: &'a AnalyzedLoop) -> Vec<&'a AnalyzedLoop> {
        match l.partition {
            Some(_) => self
                .loops
                .iter()
                .filter(|other| other.partition == l.partition)
                .collect(),
            None => vec![l],
        }
    }
}

/// One step of a loop's chain: a variable and the link from it to the next.
#[derive(Clone)]
struct ChainStep {
    /// The variable (canonical; an element subscripted).
    node: String,
    sign: LinkPolarity,
    /// The builtins the link passes through.
    via: Vec<String>,
}

/// One loop of an analysis.
#[derive(Clone)]
pub(crate) struct AnalyzedLoop {
    /// The canonical rotation of the cycle's nodes as the engine has them, a
    /// builtin's instance among them: what identifies the loop.
    key: Vec<String>,
    /// The cycle from a stock, builtin and macro internals left out.
    chain: Vec<ChainStep>,
    polarity: LoopPolarityName,
    /// The signed partition-relative score at each saved step; empty from
    /// structure.
    rel: Vec<f64>,
    /// The mean of `|rel|` over the steps its partition is active at; `None`
    /// from structure, and for a loop in no partition.
    share: Option<f64>,
    partition: Option<usize>,
    name: Option<String>,
}

impl AnalyzedLoop {
    /// Whether the loop goes through `ident` (canonical): a variable, at any
    /// element, or with a subscript one element.
    fn goes_through(&self, ident: &str) -> bool {
        match ident.split_once('[') {
            Some((variable, element)) => {
                let element = super::series::element_key(element.trim_end_matches(']'));
                let node = format!("{variable}[{element}]");
                self.chain.iter().any(|step| step.node == node)
            }
            None => self
                .chain
                .iter()
                .any(|step| strip_subscript(&step.node) == ident),
        }
    }

    /// Whether the loop has the link from `from` to `to` (canonical variable
    /// idents), at any element.
    fn has_link(&self, from: &str, to: &str) -> bool {
        let n = self.chain.len();
        (0..n).any(|i| {
            strip_subscript(&self.chain[i].node) == from
                && strip_subscript(&self.chain[(i + 1) % n].node) == to
        })
    }
}

/// What a plan's replaced equations cut.
struct Cut {
    /// Removed links, `(from, to)` by canonical ident.
    links: Vec<(String, String)>,
    loops: Vec<AnalyzedLoop>,
}

pub(crate) fn analyze_loops(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: AnalyzeLoopsInput,
) -> Result<AnalyzeLoopsOutput, ToolError> {
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;
    let name = input
        .run
        .as_deref()
        .map(str::trim)
        .unwrap_or(CURRENT)
        .to_string();
    let run = session.runs.get(ws, model, &name)?;
    if !session.runs.is_fresh(ws, &run) {
        return Err(ToolError::new(format!(
            "run '{name}' was made at revision {} and the model has changed since (revision {}); \
             run it again with run_experiment to analyze its loops",
            run.revision, ws.revision
        )));
    }
    let through = input
        .through
        .as_deref()
        .map(|variable| through_of(ws.project, model, variable))
        .transpose()?;
    if input.loops.len() > MAX_LOOPS_BY_ID {
        return Err(ToolError::new(format!(
            "ask for at most {MAX_LOOPS_BY_ID} loops by id (this call names {})",
            input.loops.len()
        )));
    }
    if !input.loops.is_empty() && through.is_some() {
        return Err(ToolError::new(
            "give loops by id or a variable they go through, not both",
        ));
    }
    let keys = input
        .loops
        .iter()
        .map(|id| {
            let id = id.trim();
            session
                .evidence
                .loop_key(id)
                .map(|key| (id.to_string(), key.to_vec()))
                .ok_or_else(|| {
                    ToolError::new(format!(
                        "no loop has the id '{id}' in this session: loop ids come from \
                         analyze_loops's answers"
                    ))
                })
        })
        .collect::<Result<Vec<(String, Vec<String>)>, ToolError>>()?;
    let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)?;
    let budget = session.outline_budget;
    Ok(if keys.is_empty() {
        report(
            &mut session.evidence,
            &analysis,
            model,
            &name,
            ws.revision,
            through,
            budget,
        )
    } else {
        report_by_id(&analysis, model, &name, ws.revision, keys, budget)
    })
}

/// What `through` names: a variable, or one element of an arrayed one, as the
/// model spells it.
pub(crate) fn through_of(
    project: &datamodel::Project,
    model: &datamodel::Model,
    name: &str,
) -> Result<String, ToolError> {
    let not_found = |suggestions: Vec<String>| {
        ToolError::new(format!(
            "the model has no variable '{}'",
            super::evidence::echo(name)
        ))
        .with_suggestions(suggestions)
    };
    if let Some(var) = model.get_variable(name) {
        return Ok(var.get_ident().to_string());
    }
    let Some((base, subscripts)) = names::split_subscript(name) else {
        return names::resolve(model, name)
            .map(|var| var.get_ident().to_string())
            .map_err(not_found);
    };
    let var = names::resolve(model, base).map_err(not_found)?;
    let dims = var
        .get_equation()
        .map(super::outline::dimensions)
        .unwrap_or_default();
    names::resolve_element(project, &dims, &subscripts)
        .map(|element| format!("{}[{element}]", var.get_ident()))
        .map_err(|reason| ToolError::new(format!("{}: {reason}", var.get_ident())))
}

/// `items` grouped by the partition each comes with, the groups in the order
/// their partitions first appear.
fn by_partition<T>(
    items: impl IntoIterator<Item = (Option<usize>, T)>,
) -> Vec<(Option<usize>, Vec<T>)> {
    let mut groups: Vec<(Option<usize>, Vec<T>)> = Vec::new();
    for (partition, item) in items {
        match groups.iter_mut().find(|(p, _)| *p == partition) {
            Some((_, members)) => members.push(item),
            None => groups.push((partition, vec![item])),
        }
    }
    groups
}

/// The answer for loops asked for by id: each whole, in its partition, within
/// `budget`, the last asked for left out for another call when they do not
/// all fit (the first always comes).
fn report_by_id(
    analysis: &LoopAnalysis,
    model: &datamodel::Model,
    run: &str,
    revision: u64,
    keys: Vec<(String, Vec<String>)>,
    budget: usize,
) -> AnalyzeLoopsOutput {
    let mut asked = keys;
    let mut answer = report_by_id_of(analysis, model, run, revision, &asked);
    super::fit(&mut answer, budget, |answer| {
        if asked.len() <= 1 {
            return false;
        }
        let Some((id, _)) = asked.pop() else {
            unreachable!("more than one loop is asked for")
        };
        let mut left_out = std::mem::take(&mut answer.left_out);
        left_out.insert(0, id);
        *answer = report_by_id_of(analysis, model, run, revision, &asked);
        answer.left_out = left_out;
        true
    });
    answer
}

fn report_by_id_of(
    analysis: &LoopAnalysis,
    model: &datamodel::Model,
    run: &str,
    revision: u64,
    keys: &[(String, Vec<String>)],
) -> AnalyzeLoopsOutput {
    let names = ModelNames::new(model);
    let mut absent = Vec::new();
    let mut reports = Vec::new();
    let (mut inactive, mut cut) = (Vec::new(), Vec::new());
    for (id, key) in keys {
        let whole = |l: &AnalyzedLoop| loop_report(l, id.clone(), &names, Detail::Whole);
        let has_key = |l: &&AnalyzedLoop| l.key == *key;
        if let Some(l) = analysis.loops.iter().find(has_key) {
            reports.push((l.partition, whole(l)));
        } else if let Some(l) = analysis.inactive.iter().flatten().find(has_key) {
            inactive.push(whole(l));
        } else if let Some(l) = analysis.cut.iter().flat_map(|c| &c.loops).find(has_key) {
            cut.push(whole(l));
        } else {
            absent.push(id.clone());
        }
    }
    let found = reports.len();
    let partitions = by_partition(reports)
        .into_iter()
        .map(|(partition, loops)| {
            let stocks = stocks_of(analysis, partition);
            PartitionReport {
                stocks: stocks
                    .iter()
                    .take(MAX_STOCKS)
                    .map(|s| names.display(s))
                    .collect(),
                other_stocks: stocks.len().saturating_sub(MAX_STOCKS),
                loop_count: analysis
                    .loops
                    .iter()
                    .filter(|l| l.partition == partition)
                    .count(),
                dominance: vec![],
                loops,
            }
        })
        .collect();
    let mut notes: Vec<String> = note(analysis, None, found, &[]).into_iter().collect();
    if !absent.is_empty() {
        notes.push(format!(
            "{} {} no loop of this run's analysis, active, inactive or cut.",
            absent.join(", "),
            if absent.len() == 1 { "is" } else { "are" }
        ));
    }
    AnalyzeLoopsOutput {
        revision,
        run: run.to_string(),
        basis: analysis.basis,
        through: None,
        found,
        complete: analysis.complete(),
        partitions,
        inactive: (!inactive.is_empty()).then_some(InactiveLoops {
            loops: inactive,
            other_loops: 0,
        }),
        omitted: None,
        absent,
        left_out: vec![],
        cut: analysis
            .cut
            .as_ref()
            .filter(|_| !cut.is_empty())
            .map(|of_plan| CutReport {
                links: of_plan
                    .links
                    .iter()
                    .map(|(from, to)| CutLink {
                        from: names.display(from),
                        to: names.display(to),
                    })
                    .collect(),
                loops: cut,
                other_loops: 0,
            }),
        note: (!notes.is_empty()).then(|| notes.join(" ")),
    }
}

/// The loop analysis of `run`: the one kept with it, or a new one.
pub(crate) fn analysis_of(
    runs: &mut RunStore,
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    source_model: SourceModel,
    run: &Arc<Run>,
) -> Result<Arc<LoopAnalysis>, ToolError> {
    if let Some(analysis) = run.loops.get() {
        return Ok(analysis.clone());
    }
    let base = if run.plan.equations.is_empty() {
        None
    } else {
        Some(model_loops(runs, ws, model, source_model)?)
    };
    let analysis = analyze(ws, model, &run.plan, base)?;
    Ok(run.loops.get_or_init(|| Arc::new(analysis)).clone())
}

/// What `analysis`'s plan cut, the loops named by `evidence`: the links its
/// replaced equations removed, and the ids of the model's loops through them.
pub(crate) fn cut_of(
    evidence: &mut Evidence,
    analysis: &LoopAnalysis,
    model: &datamodel::Model,
) -> (Vec<CutLink>, Vec<String>) {
    let Some(cut) = &analysis.cut else {
        return (vec![], vec![]);
    };
    let names = ModelNames::new(model);
    let links = cut
        .links
        .iter()
        .map(|(from, to)| CutLink {
            from: names.display(from),
            to: names.display(to),
        })
        .collect();
    let loops = cut.loops.iter().map(|l| evidence.loop_id(&l.key)).collect();
    (links, loops)
}

/// The ids of the loops that led `analysis`'s run after `time`, strongest
/// first, at most `n`: the leaders and rivals of every partition's timeline
/// spans that end after it, each by its largest share in them. None from
/// structure.
pub(crate) fn leaders_after(
    evidence: &mut Evidence,
    analysis: &LoopAnalysis,
    time: f64,
    n: usize,
) -> Vec<String> {
    if analysis.basis != LoopBasis::Run {
        return vec![];
    }
    let mut leaders: Vec<(&AnalyzedLoop, f64)> = Vec::new();
    for (partition, members) in by_partition(analysis.loops.iter().map(|l| (l.partition, l))) {
        if partition.is_none() {
            continue;
        }
        for span in timeline(&members, analysis.times.len(), |_| true) {
            // A span reaches past `time` when its last step does.
            let last = analysis.times[span.end - 1];
            if last <= time {
                continue;
            }
            for (i, share) in span.leaders {
                match leaders.iter_mut().find(|(l, _)| l.key == members[i].key) {
                    Some((_, best)) => *best = best.max(share),
                    None => leaders.push((members[i], share)),
                }
            }
        }
    }
    leaders.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.key.cmp(&b.0.key)));
    leaders
        .into_iter()
        .take(n)
        .map(|(l, _)| evidence.loop_id(&l.key))
        .collect()
}

/// The polarity the analysis gives the loop whose cycle is `key`, if it has
/// the loop.
pub(crate) fn polarity_of(analysis: &LoopAnalysis, key: &[String]) -> Option<LoopPolarityName> {
    analysis
        .loops
        .iter()
        .find(|l| l.key == key)
        .map(|l| l.polarity)
}

/// How an answer says that the loop whose cycle is `key`, one of the model's
/// loops from structure, is not among the analysis's loops of the run: it is
/// inactive in a run the analysis has every loop of, and otherwise inactive
/// or among those the analysis left out, which it cannot tell apart; `None`
/// for a loop the structure does not list either.
pub(crate) fn unreported(analysis: &LoopAnalysis, key: &[String], run: &str) -> Option<String> {
    let listed = analysis.inactive.iter().flatten().any(|l| l.key == key);
    listed.then(|| {
        if analysis.complete() {
            format!("inactive in run '{run}'")
        } else {
            format!("not among the loops this analysis reports for run '{run}'")
        }
    })
}

/// A loop's standing in its partition over a span of a run.
pub(crate) struct Leadership {
    /// Its mean share of the partition's activity over the span.
    pub share: f64,
    /// The largest mean share of any loop of the partition over the span.
    pub largest: f64,
    /// The cycle of the loop that led the span ([`strongest`]).
    pub strongest: Option<Vec<String>>,
    /// The span's steps at which the partition was active.
    pub active: usize,
}

impl Leadership {
    /// Whether the loop led the span: its mean share is the largest, or ties
    /// with it, by the rule the timeline names a span's leader by.
    pub(crate) fn leads(&self) -> bool {
        self.largest >= ACTIVE_SHARE && self.share >= self.largest * (1.0 - LEAD_TIE)
    }
}

/// The standing of the loop whose cycle is `key` in its partition over the
/// saved steps whose time, as an answer writes it ([`round`]), is from `from`
/// to `to`, both included: the span a timeline prints as `from` and `to`,
/// the times of its first and last steps, is exactly the steps the timeline
/// read. `None` when the analysis does not have the loop. A run with no
/// active loop (loops from structure) has no active step.
pub(crate) fn leadership(
    analysis: &LoopAnalysis,
    key: &[String],
    from: f64,
    to: f64,
) -> Option<Leadership> {
    let target = analysis.loops.iter().find(|l| l.key == key)?;
    let peers = analysis.peers(target);
    let series: Vec<&[f64]> = peers.iter().map(|l| l.rel.as_slice()).collect();
    let steps = analysis.times.len();
    let in_span = |i: usize| {
        let time = round(analysis.times[i]);
        time >= from && time <= to
    };
    let start = (0..steps).find(|&i| in_span(i)).unwrap_or(steps);
    let end = (start..steps).find(|&i| !in_span(i)).unwrap_or(steps);
    let means = MeanShares::new(&series, steps);
    let shares = means.means(start, end);
    let target_index = peers.iter().position(|l| l.key == key)?;
    Some(Leadership {
        share: shares[target_index],
        largest: shares.iter().copied().fold(0.0, f64::max),
        strongest: strongest(&shares, ACTIVE_SHARE).map(|i| peers[i].key.clone()),
        active: means.active(start, end),
    })
}

/// The cycles of the model's loops through the variable `ident` (canonical)
/// that the analysis knows of, each with whether it is inactive in the run:
/// its active loops, and where the structure lists the model's loops, the
/// ones the run left inactive too. `None` when those are not every loop of
/// the model ([`LoopAnalysis::shows_absence`]), so that no absence can be
/// shown.
pub(crate) fn loops_through(
    analysis: &LoopAnalysis,
    ident: &str,
) -> Option<Vec<(Vec<String>, bool)>> {
    analysis.shows_absence().then(|| {
        let active = analysis.loops.iter().map(|l| (l, false));
        let inactive = analysis.inactive.iter().flatten().map(|l| (l, true));
        active
            .chain(inactive)
            .filter(|(l, _)| l.goes_through(ident))
            .map(|(l, inactive)| (l.key.clone(), inactive))
            .collect()
    })
}

/// The loops of the model as it is, for naming what a replaced equation cuts:
/// its loops from structure when the structure alone enumerates them, else
/// those of its current run.
fn model_loops(
    runs: &mut RunStore,
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    source_model: SourceModel,
) -> Result<Vec<AnalyzedLoop>, ToolError> {
    let source_project = ws.db.current_source_project().ok_or_else(|| {
        ToolError::new("the project has not been compiled; the host must sync it first")
    })?;
    let project = ws.project;
    ws.yield_point()?;
    let structure = with_discovery_mode(ws.db, source_project, false, |db| {
        structural_loops(db, source_project, source_model, model, project)
    });
    if let Some(structure) = structure {
        return Ok(structure.loops);
    }
    let current = runs.current(ws, model)?;
    Ok(analysis_of(runs, ws, model, source_model, &current)?
        .loops
        .clone())
}

/// Why a run's loops cannot be analyzed, as a refusal.
fn unanalyzable(reason: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("the run's loops cannot be analyzed: {reason}"))
}

/// Replay `plan` under the LTM overlay in discovery mode and analyze the
/// replay's loops; `base` is the model's loops, for a plan that replaces
/// equations. It stops before each stage -- the replay, the discovery over
/// its results, the loops from structure -- when other work waits for the
/// project.
fn analyze(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    plan: &RunPlan,
    base: Option<Vec<AnalyzedLoop>>,
) -> Result<LoopAnalysis, ToolError> {
    let source_project = ws
        .db
        .current_source_project()
        .ok_or_else(|| unanalyzable("the project has not been compiled"))?;
    let canonical = crate::canonicalize(&model.name).into_owned();
    let source_model = source_project
        .models(ws.db)
        .get(&canonical)
        .copied()
        .ok_or_else(|| unanalyzable(format!("the project has no model named '{}'", model.name)))?;
    let replaced: Vec<&str> = plan
        .equations
        .iter()
        .map(|change| change.variable.as_str())
        .collect();
    let read_before = inputs(ws.db, source_model, source_project, &replaced);
    let project = ws.project;
    let (waiting, cancelled) = (ws.waiting, ws.cancelled);
    ws.yield_point()?;
    // The mode is an input on the project's handle, which a staged sync
    // keeps, so the staged copy compiles in it too. It is set back when the
    // guard drops, however the run ends.
    let mut mode = DiscoveryMode::set(ws.db, source_project, true);
    let mut inner = Workspace {
        project: ws.project,
        db: &mut mode,
        revision: ws.revision,
        waiting,
        cancelled,
    };
    let outcome = runs::execute_then(
        &mut inner,
        model,
        plan,
        LtmOverlay::On,
        |db, source_project, results| {
            let mut analysis =
                read_loops(db, source_project, model, project, plan, results, waiting)?;
            if let Some(base) = base {
                let source_model = source_project.models(db).get(&canonical).copied();
                let read_after = source_model
                    .map(|m| inputs(db, m, source_project, &replaced))
                    .unwrap_or_default();
                analysis.cut = Some(cut(read_before, &read_after, base));
            }
            Ok(analysis)
        },
    );
    drop(mode);
    let (_results, analysis) = outcome.map_err(|failure| failure.refusal(unanalyzable))?;
    analysis
}

/// The project's LTM discovery mode set, until the guard drops, which sets it
/// back whatever happened meanwhile, a panic included: an unwinding host
/// would otherwise keep a project in discovery mode, whose structural
/// surface reports pinned loops only.
struct DiscoveryMode<'a> {
    db: &'a mut SimlinDb,
    source_project: SourceProject,
    prior: bool,
}

impl<'a> DiscoveryMode<'a> {
    fn set(db: &'a mut SimlinDb, source_project: SourceProject, enabled: bool) -> Self {
        let prior = source_project.ltm_discovery_mode(db);
        set_project_ltm_discovery_mode(db, source_project, enabled);
        DiscoveryMode {
            db,
            source_project,
            prior,
        }
    }
}

impl std::ops::Deref for DiscoveryMode<'_> {
    type Target = SimlinDb;
    fn deref(&self) -> &SimlinDb {
        self.db
    }
}

impl std::ops::DerefMut for DiscoveryMode<'_> {
    fn deref_mut(&mut self) -> &mut SimlinDb {
        self.db
    }
}

impl Drop for DiscoveryMode<'_> {
    fn drop(&mut self) {
        set_project_ltm_discovery_mode(self.db, self.source_project, self.prior);
    }
}

/// Run `f` with the project's LTM discovery mode set to `enabled`, and set it
/// back after.
fn with_discovery_mode<T>(
    db: &mut SimlinDb,
    source_project: SourceProject,
    enabled: bool,
    f: impl FnOnce(&mut SimlinDb) -> T,
) -> T {
    let mut mode = DiscoveryMode::set(db, source_project, enabled);
    f(&mut mode)
}

/// The loops of a run of `model` under `plan` compiled under the LTM overlay
/// in discovery mode, from its `results`, with the model's other loops from
/// structure as inactive; the model's loops from structure when none was
/// active, or when its stocks move by nothing but residue. Stops before
/// discovery, and before the loops from structure, when `waiting` says other
/// work waits for the project.
fn read_loops(
    db: &mut SimlinDb,
    source_project: SourceProject,
    model: &datamodel::Model,
    project: &datamodel::Project,
    plan: &RunPlan,
    results: &Results,
    waiting: Option<&(dyn Fn() -> bool + Sync)>,
) -> Result<LoopAnalysis, ToolError> {
    let yield_point = || match waiting {
        Some(waiting) if waiting() => Err(ToolError::interrupted()),
        _ => Ok(()),
    };
    let canonical = crate::canonicalize(&model.name).into_owned();
    let source_model = source_project
        .models(db)
        .get(&canonical)
        .copied()
        .ok_or_else(|| unanalyzable(format!("the project has no model named '{}'", model.name)))?;
    yield_point()?;
    let discovery = crate::analysis::discover_run_loops(
        db,
        source_project,
        &canonical,
        results,
        Some(DISCOVERY_BUDGET),
    )
    .ok_or_else(|| unanalyzable("loop discovery could not read the model's structure"))?;

    let names = ModelNames::new(model);
    let loop_names = LoopNames::new(model, project);
    let mut loops: Vec<AnalyzedLoop> = {
        let signs = RunSigns::new(db, source_model, source_project, results);
        discovery
            .loops
            .iter()
            .map(|found| {
                let equation_sign: HashMap<(&str, &str), LinkPolarity> = found
                    .loop_info
                    .links
                    .iter()
                    .map(|link| ((link.from.as_str(), link.to.as_str()), link.polarity))
                    .collect();
                let nodes = crate::db::loop_node_sequence(&found.loop_info);
                let chain = names.chain(&nodes, |from, to| {
                    signs.sign(from, to).unwrap_or_else(|| {
                        equation_sign
                            .get(&(from, to))
                            .copied()
                            .unwrap_or(LinkPolarity::Unknown)
                    })
                });
                AnalyzedLoop {
                    name: loop_names.of(&chain),
                    key: canonical_rotation(&nodes),
                    chain,
                    polarity: found.loop_info.polarity.into(),
                    rel: found
                        .rel_scores
                        .iter()
                        .map(|s| if s.is_finite() { *s } else { 0.0 })
                        .collect(),
                    share: None,
                    partition: found.partition,
                }
            })
            .collect()
    };
    set_shares(&mut loops, results.step_count);

    // The model's loops from structure: every loop of it when none was
    // active in the run, the inactive ones otherwise.
    yield_point()?;
    let structure = with_discovery_mode(db, source_project, false, |db| {
        structural_loops(db, source_project, source_model, model, project)
    });
    let enumerated = structure.is_some();
    // A run whose stocks move by nothing but the rounding of what they are
    // computed from has loop scores all the same, since a score is a ratio
    // of changes however small; those of residue are noise, and so would be
    // any dominance read from them. A real movement, however small, is
    // analyzed from its scores.
    if !loops.is_empty() && !stocks_move_by_residue(model, plan, results) {
        let inactive = structure.map(|structure| {
            let active: HashSet<&Vec<String>> = loops.iter().map(|l| &l.key).collect();
            let mut inactive: Vec<AnalyzedLoop> = structure
                .loops
                .into_iter()
                .filter(|l| !active.contains(&l.key))
                .collect();
            inactive.sort_by(|a, b| a.chain.len().cmp(&b.chain.len()).then(a.key.cmp(&b.key)));
            inactive
        });
        return Ok(LoopAnalysis {
            basis: LoopBasis::Run,
            times: results
                .iter()
                .map(|row| row[crate::results::TIME_OFF])
                .collect(),
            loops,
            partitions: discovery
                .partitions
                .iter()
                .map(|p| p.stocks.clone())
                .collect(),
            sampled: !discovery.enumeration_complete
                || discovery.agg_recovery_truncated
                || discovery.truncated,
            capped_from: (discovery.retained_loops > discovery.loops.len())
                .then_some(discovery.retained_loops),
            negligible: discovery.universe_loops.map_or(0, |universe| {
                universe.saturating_sub(discovery.retained_loops)
            }),
            enumerated,
            conveyors: false,
            inactive,
            cut: None,
        });
    }

    // A conveyor or queue builds through a path that synthesizes no loop
    // scores, and the causal graph has no link through one.
    let conveyors = crate::conveyor_compile::project_has_conveyor(project, &model.name)
        || crate::queue_compile::project_has_queue(project, &model.name);
    let structure = structure.unwrap_or_default();
    Ok(LoopAnalysis {
        basis: LoopBasis::Structure,
        times: vec![],
        loops: structure.loops,
        partitions: structure.partitions,
        sampled: false,
        capped_from: None,
        negligible: 0,
        enumerated,
        conveyors,
        inactive: None,
        cut: None,
    })
}

/// Whether every stock of `model`, each element of an arrayed one, never
/// leaves zero in `results`, a run under `plan`, by more than residue of the
/// scale it is computed from in the run (`behavior::residue_bound` at
/// `series::scale_in_run`, the scale every tool reads it at). False for a
/// model with no stock.
fn stocks_move_by_residue(model: &datamodel::Model, plan: &RunPlan, results: &Results) -> bool {
    let mut any = false;
    for var in &model.variables {
        let Variable::Stock(stock) = var else {
            continue;
        };
        let canonical = crate::canonicalize(&stock.ident).into_owned();
        for column in super::series::variable_columns(results, model, &canonical) {
            any = true;
            let series: Vec<f64> = results.iter().map(|row| row[column.offset]).collect();
            let key = format!("{canonical}{}", column.subscript);
            let scale = super::series::scale_in_run(results, model, plan, &key);
            if super::behavior::magnitude(&series) > super::behavior::residue_bound(scale) {
                return false;
            }
        }
    }
    any
}

/// Give each loop of a run its mean share of its partition's activity, over
/// the steps the partition is active at. A loop in no partition competes with
/// none, so it has no share.
fn set_shares(loops: &mut [AnalyzedLoop], steps: usize) {
    let groups = by_partition(loops.iter().enumerate().map(|(i, l)| (l.partition, i)));
    for (partition, members) in groups {
        if partition.is_none() {
            continue;
        }
        let shares: Vec<f64> = {
            let series: Vec<&[f64]> = members.iter().map(|&i| loops[i].rel.as_slice()).collect();
            MeanShares::new(&series, steps).means(0, steps)
        };
        for (i, share) in members.into_iter().zip(shares) {
            loops[i].share = Some(share);
        }
    }
}

/// The signs a run under the LTM overlay gave its links, by `(from, to)`
/// element link, to sign a discovered loop's links with.
struct RunSigns<'a> {
    results: &'a Results,
    /// Each scored link's column in the results.
    offsets: HashMap<(String, String), usize>,
    /// The columns of the scored links into each node, in column order: what
    /// a link's relative score is a share of.
    into: HashMap<String, Vec<usize>>,
    /// The links out of each aggregate node, the synthetic node the engine
    /// routes an array reducer (`SUM(pop[*])`) through.
    out_of_aggregate: HashMap<String, Vec<(String, usize)>>,
    /// The signs already read.
    read: RefCell<HashMap<(String, String), Option<LinkPolarity>>>,
}

impl<'a> RunSigns<'a> {
    /// The longest chain of aggregate nodes a link is signed across: a
    /// reducer of a reducer's result is two.
    const MAX_AGGREGATES: usize = 4;

    fn new(
        db: &SimlinDb,
        source_model: SourceModel,
        source_project: SourceProject,
        results: &'a Results,
    ) -> RunSigns<'a> {
        let ltm_vars = crate::db::model_ltm_variables(db, source_model, source_project);
        let dims = crate::db::project_datamodel_dims(db, source_project);
        let expansion =
            crate::analysis::build_link_expansion_context(db, source_model, source_project);
        let offsets: HashMap<(String, String), usize> =
            crate::ltm_finding::link_score_offsets(results, &ltm_vars.vars, dims, &expansion)
                .into_iter()
                .map(|((from, to), offset)| {
                    ((from.as_str().to_string(), to.as_str().to_string()), offset)
                })
                .collect();
        let mut into: HashMap<String, Vec<usize>> = HashMap::new();
        let mut out_of_aggregate: HashMap<String, Vec<(String, usize)>> = HashMap::new();
        for ((from, to), &offset) in &offsets {
            into.entry(to.clone()).or_default().push(offset);
            if is_aggregate(from) {
                out_of_aggregate
                    .entry(from.clone())
                    .or_default()
                    .push((to.clone(), offset));
            }
        }
        // A sum of floats depends on its order, and a map's does not hold.
        for columns in into.values_mut() {
            columns.sort_unstable();
        }
        for links in out_of_aggregate.values_mut() {
            links.sort();
        }
        RunSigns {
            results,
            offsets,
            into,
            out_of_aggregate,
            read: RefCell::new(HashMap::new()),
        }
    }

    /// The relative score series of the link in column `offset` into `to`:
    /// its share, signed, of the change all of `to`'s scored inputs account
    /// for at each step (`ltm_post`'s normalization, per target).
    fn relative(&self, offset: usize, to: &str) -> Vec<f64> {
        let column = |c: usize| self.results.iter().map(move |row| row[c]);
        let inputs = self.into.get(to).map(Vec::as_slice).unwrap_or(&[]);
        let totals = crate::ltm_post::group_totals(
            inputs.iter().map(|&c| ((), column(c))),
            self.results.step_count,
        );
        let totals = totals.get(&()).map(Vec::as_slice).unwrap_or(&[]);
        crate::ltm_post::relative_series(column(offset), totals)
    }

    /// The sign of the link `from -> to` in the run
    /// ([`LinkPolarity::from_runtime_scores`] over its relative score), or,
    /// for a link discovery stitched across aggregate nodes, the sign of the
    /// path through them (the product of the relative scores along it) when
    /// every such path has the same one.
    ///
    /// `Unknown` when the link was active and its sign changed over the run,
    /// as a loop's polarity is undetermined, so a loop the run leaves
    /// undetermined shows which of its links did. `None` when the run never
    /// scored the link, and its equation's sign stands.
    fn sign(&self, from: &str, to: &str) -> Option<LinkPolarity> {
        let link = (from.to_string(), to.to_string());
        if let Some(sign) = self.read.borrow().get(&link) {
            return *sign;
        }
        let paths = match self.offsets.get(&link) {
            Some(&offset) => vec![vec![(offset, to)]],
            None => self.paths(from, to),
        };
        let mut signs = paths.iter().filter_map(|path| {
            let mut along = vec![1.0_f64; self.results.step_count];
            for &(offset, target) in path {
                for (product, score) in along.iter_mut().zip(self.relative(offset, target)) {
                    *product *= score;
                }
            }
            LinkPolarity::from_runtime_scores(&along)
        });
        let sign = signs.next().map(|first| {
            if signs.all(|sign| sign == first) {
                first
            } else {
                LinkPolarity::Unknown
            }
        });
        self.read.borrow_mut().insert(link, sign);
        sign
    }

    /// Each path from `from` to `to` through aggregate nodes only, as the
    /// column and target of each link along it.
    fn paths<'s>(&'s self, from: &str, to: &'s str) -> Vec<Vec<(usize, &'s str)>> {
        let mut found = Vec::new();
        let mut first: Vec<(&str, usize)> = self
            .offsets
            .iter()
            .filter(|((source, node), _)| source == from && is_aggregate(node))
            .map(|((_, node), &offset)| (node.as_str(), offset))
            .collect();
        first.sort();
        for (node, offset) in first {
            self.extend(node, to, vec![(offset, node)], &mut found);
        }
        found
    }

    fn extend<'s>(
        &'s self,
        node: &str,
        to: &'s str,
        path: Vec<(usize, &'s str)>,
        found: &mut Vec<Vec<(usize, &'s str)>>,
    ) {
        for (next, offset) in self.out_of_aggregate.get(node).into_iter().flatten() {
            let mut longer = path.clone();
            if next == to {
                longer.push((*offset, to));
                found.push(longer);
            } else if is_aggregate(next) && longer.len() < Self::MAX_AGGREGATES {
                longer.push((*offset, next.as_str()));
                self.extend(next, to, longer, found);
            }
        }
    }
}

/// Whether a loop node is an aggregate node, one discovery collapses out of
/// the loops it reports.
fn is_aggregate(node: &str) -> bool {
    crate::ltm_agg::is_synthetic_agg_name(strip_subscript(node))
}

/// A model's loops from its structure, and their partitions.
#[derive(Default)]
struct Structure {
    loops: Vec<AnalyzedLoop>,
    partitions: Vec<Vec<String>>,
}

/// The model's loops from its structure, with their equations' signs: `None`
/// when the model is too large for its structure alone to enumerate them.
/// The project must be out of discovery mode, in which the structural surface
/// reports pinned loops only.
///
/// A loop is one cycle of the model's elements, as a run's loops are, so a
/// loop has one key from structure and from a run: the structural surface
/// reports an apply-to-all loop once, by its variables, and it is expanded
/// here to the loop of each element ([`ModelNames::element_cycles`]). A
/// loop's partition is the one its stocks are in
/// (`db::model_element_cycle_partitions`), which each element's loop has its
/// own of when the elements are not coupled.
fn structural_loops(
    db: &SimlinDb,
    source_project: SourceProject,
    source_model: SourceModel,
    model: &datamodel::Model,
    project: &datamodel::Project,
) -> Option<Structure> {
    if crate::db::model_ltm_mode(db, source_model, source_project) != LtmMode::Exhaustive {
        return None;
    }
    let detected = crate::db::model_detected_loops(db, source_model, source_project);
    if detected.loops.is_empty() {
        return Some(Structure::default());
    }
    let polarities = crate::db::compute_link_polarities(db, source_model, source_project);
    let stock_partitions =
        crate::db::model_element_cycle_partitions(db, source_model, source_project);
    let names = ModelNames::new(model);
    let loop_names = LoopNames::new(model, project);
    let mut partitions: Vec<Vec<String>> = Vec::new();
    let mut listed: HashMap<usize, usize> = HashMap::new();
    let mut loops = Vec::new();
    for detected in &detected.loops {
        for nodes in names.element_cycles(&detected.variables, &project.dimensions) {
            let chain = names.chain(&nodes, |from, to| {
                polarities
                    .get(&(
                        strip_subscript(from).to_string(),
                        strip_subscript(to).to_string(),
                    ))
                    .copied()
                    .unwrap_or(LinkPolarity::Unknown)
            });
            let partition = nodes
                .iter()
                .find_map(|node| stock_partitions.stock_partition.get(node))
                .map(|&of_model| {
                    *listed.entry(of_model).or_insert_with(|| {
                        let mut stocks = stock_partitions.partitions[of_model].clone();
                        stocks.sort();
                        partitions.push(stocks);
                        partitions.len() - 1
                    })
                });
            let key = canonical_rotation(&nodes);
            let polarity: LoopPolarityName = detected.polarity.into();
            // The structural surface can report one cycle of elements twice
            // (an apply-to-all loop through a reducer beside the direct one
            // through the same elements, the aggregate node left out): one
            // loop, whose sign is known only where both say the same.
            if let Some(twin) = loops.iter_mut().find(|l: &&mut AnalyzedLoop| l.key == key) {
                if twin.polarity != polarity {
                    twin.polarity = LoopPolarityName::Undetermined;
                }
                continue;
            }
            loops.push(AnalyzedLoop {
                name: detected.name.clone().or_else(|| loop_names.of(&chain)),
                key,
                chain,
                polarity,
                rel: vec![],
                share: None,
                partition,
            });
        }
    }
    Some(Structure { loops, partitions })
}

/// The variables each of `variables` (canonical idents) reads, as
/// `(from, to)` links.
fn inputs(
    db: &SimlinDb,
    source_model: SourceModel,
    source_project: SourceProject,
    variables: &[&str],
) -> Vec<(String, String)> {
    if variables.is_empty() {
        return vec![];
    }
    crate::analysis::model_links(db, source_model, source_project, None, false)
        .into_iter()
        .filter(|link| variables.contains(&link.to.as_str()))
        .map(|link| (link.from, link.to))
        .collect()
}

/// The links `before` has and `after` does not, and the loops of `base`
/// through them.
fn cut(before: Vec<(String, String)>, after: &[(String, String)], base: Vec<AnalyzedLoop>) -> Cut {
    let mut links: Vec<(String, String)> = before
        .into_iter()
        .filter(|link| !after.contains(link))
        .collect();
    links.sort();
    links.dedup();
    let mut loops: Vec<AnalyzedLoop> = base
        .into_iter()
        .filter(|l| links.iter().any(|(from, to)| l.has_link(from, to)))
        .collect();
    loops.sort_by(|a, b| a.chain.len().cmp(&b.chain.len()).then(a.key.cmp(&b.key)));
    Cut { links, loops }
}

/// The builtin or macro a node the engine synthesized is an instance of
/// (`smth1` for `$⁚perceived⁚0⁚smth1`, the spelling of
/// `capture::synthetic_ident`: parent, call number, part, and an element for
/// a per-element helper); `None` for a call's hoisted argument or a capture
/// (`arg{n}`), which is part of its call's link and names nothing, and for an
/// aggregate node.
fn instance_of(node: &str) -> Option<&str> {
    let name = strip_subscript(node);
    if crate::ltm_agg::is_synthetic_agg_name(name) || !is_synthetic_node_name(name) {
        return None;
    }
    let instance = name.split('\u{00B7}').next()?;
    let part = instance.split('\u{205A}').nth(3)?;
    let is_argument = part
        .strip_prefix("arg")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!is_argument).then_some(part)
}

/// A share rounded to hundredths.
pub(crate) fn rounded_share(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// How the model names its variables, by canonical ident.
struct ModelNames<'a> {
    by_ident: HashMap<String, &'a Variable>,
}

impl<'a> ModelNames<'a> {
    fn new(model: &'a datamodel::Model) -> ModelNames<'a> {
        ModelNames {
            by_ident: model
                .variables
                .iter()
                .map(|var| (crate::canonicalize(var.get_ident()).into_owned(), var))
                .collect(),
        }
    }

    /// A loop node as the model names it: the variable's own spelling, with
    /// an element's subscript as the engine writes it.
    fn display(&self, node: &str) -> String {
        let variable = strip_subscript(node);
        match self.by_ident.get(variable) {
            Some(var) => format!("{}{}", var.get_ident(), &node[variable.len()..]),
            None => node.to_string(),
        }
    }

    fn is_stock(&self, node: &str) -> bool {
        matches!(
            self.by_ident.get(strip_subscript(node)),
            Some(Variable::Stock(_))
        )
    }

    /// The dimensions the model declares the variable `ident` over; none for
    /// a scalar, a module, and a node that is no variable of the model.
    fn dimensions(&self, ident: &str) -> Vec<String> {
        self.by_ident
            .get(ident)
            .and_then(|var| var.get_equation())
            .map(super::outline::dimensions)
            .unwrap_or_default()
    }

    /// The cycles of elements the structural loop through `nodes` stands for.
    /// The structural surface names a loop that every element of an
    /// apply-to-all family has its own of by the family's variables, without
    /// subscripts; that is one loop per element of the variables'
    /// dimensions, each arrayed node subscripted with it. A loop whose nodes
    /// carry elements already, and a loop of scalars, is itself.
    fn element_cycles(
        &self,
        nodes: &[String],
        dimensions: &[datamodel::Dimension],
    ) -> Vec<Vec<String>> {
        let itself = || vec![nodes.to_vec()];
        // A node that carries its element is no name the model declares, so
        // has no dimensions here: a loop of elements is itself.
        let Some(over) = nodes
            .iter()
            .map(|node| self.dimensions(node))
            .find(|dims| !dims.is_empty())
        else {
            return itself();
        };
        let elements = crate::ltm::loop_dimension_element_tuples(&over, dimensions);
        if elements.is_empty() {
            return itself();
        }
        elements
            .iter()
            .map(|element| {
                nodes
                    .iter()
                    .map(|node| {
                        if self.dimensions(node).is_empty() {
                            node.clone()
                        } else {
                            format!("{node}[{element}]")
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// The loop through `nodes` (its node sequence, as the engine reports
    /// it) as a report gives it: from a stock, the engine's synthetic nodes
    /// (a builtin's or macro's internals) left out, each link signed by
    /// `sign(from, to)` over the links of `nodes`, a link across synthetic
    /// nodes by composing the links it stands for and marked with the
    /// builtins among them.
    fn chain(&self, nodes: &[String], sign: impl Fn(&str, &str) -> LinkPolarity) -> Vec<ChainStep> {
        let n = nodes.len();
        let visible = |node: &String| !is_synthetic_node_name(node);
        // From a stock; else from a variable, which a chain has to start at
        // for the links across the synthetic nodes to have one to belong to.
        let Some(start) = nodes
            .iter()
            .position(|node| self.is_stock(node))
            .or_else(|| nodes.iter().position(visible))
        else {
            return (0..n)
                .map(|i| ChainStep {
                    node: nodes[i].clone(),
                    sign: sign(&nodes[i], &nodes[(i + 1) % n]),
                    via: vec![],
                })
                .collect();
        };
        let mut chain: Vec<ChainStep> = Vec::with_capacity(n);
        for i in (start..n).chain(0..start) {
            let (from, to) = (&nodes[i], &nodes[(i + 1) % n]);
            let link = sign(from, to);
            match chain.last_mut() {
                Some(last) if !visible(from) => {
                    last.sign = last.sign.compose(link);
                    if let Some(builtin) = instance_of(from)
                        && !last.via.iter().any(|via| via == builtin)
                    {
                        last.via.push(builtin.to_string());
                    }
                }
                _ => chain.push(ChainStep {
                    node: from.clone(),
                    sign: link,
                    via: vec![],
                }),
            }
        }
        chain
    }
}

/// The names a model gives its loops.
struct LoopNames<'a> {
    model: &'a datamodel::Model,
    project: &'a datamodel::Project,
    by_uids: Vec<(Vec<i32>, String)>,
}

impl<'a> LoopNames<'a> {
    fn new(model: &'a datamodel::Model, project: &'a datamodel::Project) -> LoopNames<'a> {
        LoopNames {
            model,
            project,
            by_uids: crate::analysis::build_uid_to_loop_name(project, &model.name),
        }
    }

    /// The name of the loop through `chain`, if the model gives it one.
    fn of(&self, chain: &[ChainStep]) -> Option<String> {
        let variables: HashSet<String> = chain
            .iter()
            .map(|step| strip_subscript(&step.node).to_string())
            .collect();
        crate::analysis::persisted_loop_name(
            &variables,
            &self.by_uids,
            &self.model.name,
            self.project,
        )
    }
}

/// A timeline span over saved steps `[start, end)`: its leader, then its
/// rivals, as indices into the partition's loops with their shares over it.
struct Span {
    start: usize,
    end: usize,
    leaders: Vec<(usize, f64)>,
}

/// The dominance timeline of a partition's loops over a run of `steps` saved
/// steps: the run cut where the lead changes, in at most [`MAX_SPANS`] spans
/// ([`leader_timeline`] over [`leader_by_step`]), each span led by the loop
/// with the largest mean share over its steps ([`strongest`]), so a span
/// joined from several is led by what its shares show. The loops are taken
/// in key order, so a tie goes to the same loop however a caller lists them.
/// Each span names its leader and its rivals: the loops whose share over the
/// span is at least [`RIVAL_SHARE`] of the leader's, strongest first, at most
/// [`MAX_LEADERS`] in all. Only the loops `counts` says count are named: a
/// span another loop leads is led by none of them, and neighbouring spans led
/// by none are one.
fn timeline(loops: &[&AnalyzedLoop], steps: usize, counts: impl Fn(usize) -> bool) -> Vec<Span> {
    let mut order: Vec<usize> = (0..loops.len()).collect();
    order.sort_by(|&a, &b| loops[a].key.cmp(&loops[b].key));
    let series: Vec<&[f64]> = order.iter().map(|&i| loops[i].rel.as_slice()).collect();
    let means = MeanShares::new(&series, steps);
    let shares = |start: usize, end: usize| means.means(start, end);
    let leaders = leader_by_step(&series, steps, ACTIVE_SHARE);
    let mut spans: Vec<Span> = Vec::new();
    for span in leader_timeline(&leaders, MAX_SPANS, |start, end| {
        strongest(&shares(start, end), ACTIVE_SHARE)
    }) {
        let leader = span.leader.map(|o| order[o]).filter(|&i| counts(i));
        match spans.last_mut() {
            Some(last) if leader.is_none() && last.leaders.is_empty() => last.end = span.end,
            _ => {
                // Each loop's share, by its place in `loops`.
                let mut share_of = vec![0.0; loops.len()];
                for (o, share) in shares(span.start, span.end).into_iter().enumerate() {
                    share_of[order[o]] = share;
                }
                let mut named = Vec::new();
                if let Some(leader) = leader {
                    let leads = share_of[leader];
                    let mut rivals: Vec<(usize, f64)> = order
                        .iter()
                        .map(|&i| (i, share_of[i]))
                        .filter(|&(i, s)| {
                            i != leader
                                && counts(i)
                                && s >= ACTIVE_SHARE
                                && s >= RIVAL_SHARE * leads
                        })
                        .collect();
                    // Strongest first, a tie in key order (`order`'s).
                    rivals.sort_by(|a, b| b.1.total_cmp(&a.1));
                    rivals.truncate(MAX_LEADERS - 1);
                    named.push((leader, leads));
                    named.extend(rivals);
                }
                spans.push(Span {
                    start: span.start,
                    end: span.end,
                    leaders: named,
                });
            }
        }
    }
    spans
}

/// The answer for `analysis`, its loops named by `evidence`, within `budget`
/// bytes: the longest prefix of what it would list ([`Selection`]) that fits.
/// The fit is made on the ids the loops would be given, and only the answer's
/// loops are named, so a loop left out has no id.
fn report(
    evidence: &mut Evidence,
    analysis: &LoopAnalysis,
    model: &datamodel::Model,
    run: &str,
    revision: u64,
    through: Option<String>,
    budget: usize,
) -> AnalyzeLoopsOutput {
    let names = ModelNames::new(model);
    let through_ident = through
        .as_deref()
        .map(|name| crate::canonicalize(name).into_owned());
    let selection = Selection::new(analysis, through_ident.as_deref());
    let answer = |units: usize, rivals: bool, id: &mut dyn FnMut(&[String]) -> String| {
        let body = selection.build(units, rivals, id, analysis, &names);
        AnalyzeLoopsOutput {
            revision,
            run: run.to_string(),
            basis: analysis.basis,
            through: through.clone(),
            found: selection.found,
            complete: analysis.complete(),
            partitions: body.partitions,
            inactive: body.inactive,
            omitted: body.omitted,
            absent: vec![],
            left_out: vec![],
            cut: body.cut,
            note: note(
                analysis,
                through.as_deref(),
                selection.found,
                &selection.spread(units, analysis, &names),
            ),
        }
    };
    let trial = |units: usize, rivals: bool| {
        let mut preview = evidence.preview_loop_ids();
        answer(units, rivals, &mut |key| preview.loop_id(key))
    };
    // One unit fewer at a time; the first partition's timeline and leaders
    // always come, without the rivals when even that is over the budget.
    let (mut units, mut rivals) = (selection.units(), true);
    let mut fitted = trial(units, rivals);
    super::fit(&mut fitted, budget, |fitted| {
        if units > 1 {
            units -= 1;
        } else if rivals {
            rivals = false;
        } else {
            return false;
        }
        *fitted = trial(units, rivals);
        true
    });
    answer(units, rivals, &mut |key| evidence.loop_id(key))
}

/// What an answer would list, in the order it is worth listing: one unit for
/// each partition (its stocks, its timeline, and the loops the timeline
/// names), the largest partition first; then the loops a cut names; then the
/// partitions' other loops, the most important first; then the model's
/// inactive loops. An answer lists a prefix of the units.
struct Selection<'a> {
    /// The partitions an answer may list: those with a matching loop, the
    /// largest first, at most [`MAX_PARTITIONS`].
    groups: Vec<Group<'a>>,
    /// How many partitions have a matching loop.
    partitions: usize,
    /// The loops the analysis has that match.
    found: usize,
    /// The inactive loops that match, shortest first.
    inactive: Vec<&'a AnalyzedLoop>,
    /// The units after the partitions'.
    extras: Vec<Extra>,
}

/// A loop an answer lists past its partitions' leaders.
#[derive(Clone, Copy)]
enum Extra {
    /// The cut's loop at this index.
    Cut(usize),
    /// A partition's loop its timeline does not name.
    Other { group: usize, member: usize },
    /// The inactive loop at this index.
    Inactive(usize),
}

/// A partition an answer may list.
struct Group<'a> {
    partition: Option<usize>,
    /// Its loops, most important first.
    members: Vec<&'a AnalyzedLoop>,
    /// Whether each of `members` matches.
    matching: Vec<bool>,
    spans: Vec<Span>,
}

impl Group<'_> {
    /// The members the timeline names, most important first: every span's
    /// leader and rivals, or without `rivals` its leader alone.
    fn named(&self, rivals: bool) -> Vec<usize> {
        let per_span = if rivals { MAX_LEADERS } else { 1 };
        let mut named: Vec<usize> = self
            .spans
            .iter()
            .flat_map(|span| span.leaders.iter().take(per_span).map(|&(i, _)| i))
            .collect();
        named.sort_unstable();
        named.dedup();
        named
    }
}

/// What [`Selection::build`] makes: the answer's parts that depend on the
/// selection.
struct Body {
    partitions: Vec<PartitionReport>,
    inactive: Option<InactiveLoops>,
    omitted: Option<OmittedLoops>,
    cut: Option<CutReport>,
}

impl<'a> Selection<'a> {
    fn new(analysis: &'a LoopAnalysis, through: Option<&str>) -> Selection<'a> {
        let matches = |l: &AnalyzedLoop| through.is_none_or(|ident| l.goes_through(ident));

        // The loops of each partition; loops in no partition last.
        let mut groups = by_partition(analysis.loops.iter().map(|l| (l.partition, l)));
        groups.retain(|(_, members)| members.iter().any(|l| matches(l)));
        groups.sort_by(|(a, _), (b, _)| {
            let (a_stocks, b_stocks) = (stocks_of(analysis, *a), stocks_of(analysis, *b));
            a.is_none()
                .cmp(&b.is_none())
                .then(b_stocks.len().cmp(&a_stocks.len()))
                .then(a_stocks.cmp(b_stocks))
        });

        let found = analysis.loops.iter().filter(|l| matches(l)).count();
        let partitions = groups.len();
        let groups: Vec<Group<'a>> = groups
            .into_iter()
            .take(MAX_PARTITIONS)
            .map(|(partition, mut members)| {
                // Most important first: by share from a run, shortest from
                // structure.
                members.sort_by(|a, b| {
                    b.share
                        .unwrap_or(0.0)
                        .total_cmp(&a.share.unwrap_or(0.0))
                        .then(a.chain.len().cmp(&b.chain.len()))
                        .then(a.key.cmp(&b.key))
                });
                let matching: Vec<bool> = members.iter().map(|l| matches(l)).collect();
                // Loops in no partition do not compete, so they have no
                // timeline.
                let spans = match (analysis.basis, partition) {
                    (LoopBasis::Run, Some(_)) => {
                        timeline(&members, analysis.times.len(), |i| matching[i])
                    }
                    (LoopBasis::Run, None) | (LoopBasis::Structure, _) => vec![],
                };
                Group {
                    partition,
                    members,
                    matching,
                    spans,
                }
            })
            .collect();

        let inactive: Vec<&AnalyzedLoop> = analysis
            .inactive
            .iter()
            .flatten()
            .filter(|l| matches(l))
            .collect();

        let cut = analysis.cut.as_ref().map_or(0, |cut| cut.loops.len());
        let mut extras: Vec<Extra> = (0..cut.min(MAX_LOOPS)).map(Extra::Cut).collect();
        // Each partition's first few others, the partitions' taken together
        // by share, then by standing within their partition.
        let mut others: Vec<(f64, usize, usize, usize)> = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            let named = group.named(true);
            let unnamed = (0..group.members.len())
                .filter(|i| group.matching[*i] && !named.contains(i))
                .take(MAX_LOOPS);
            for (rank, member) in unnamed.enumerate() {
                others.push((group.members[member].share.unwrap_or(0.0), rank, g, member));
            }
        }
        others.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));
        extras.extend(
            others
                .into_iter()
                .map(|(_, _, group, member)| Extra::Other { group, member }),
        );
        extras.extend((0..inactive.len().min(MAX_LOOPS)).map(Extra::Inactive));

        Selection {
            groups,
            partitions,
            found,
            inactive,
            extras,
        }
    }

    /// How many units there are to list.
    fn units(&self) -> usize {
        self.groups.len() + self.extras.len()
    }

    /// The first stock of each of the partitions the first `units` units list
    /// in which, over some span, the leader does not dominate.
    fn spread(&self, units: usize, analysis: &LoopAnalysis, names: &ModelNames<'_>) -> Vec<String> {
        self.groups
            .iter()
            .take(units)
            .filter(|g| {
                g.spans.iter().any(|span| {
                    span.leaders
                        .first()
                        .is_some_and(|&(_, share)| share < DOMINANT_SHARE)
                })
            })
            .filter_map(|g| stocks_of(analysis, g.partition).first())
            .map(|stock| names.display(stock))
            .collect()
    }

    /// The answer's parts for the first `units` units, with each span's
    /// rivals or without, the loops they list named by `id` in the order
    /// they are listed.
    fn build(
        &self,
        units: usize,
        rivals: bool,
        id: &mut dyn FnMut(&[String]) -> String,
        analysis: &LoopAnalysis,
        names: &ModelNames<'_>,
    ) -> Body {
        let extras = &self.extras[..units
            .saturating_sub(self.groups.len())
            .min(self.extras.len())];
        let per_span = if rivals { MAX_LEADERS } else { 1 };
        let mut listed = 0;
        let partitions: Vec<PartitionReport> = self
            .groups
            .iter()
            .enumerate()
            .take(units.max(1))
            .map(|(g, group)| {
                let mut members = group.named(rivals);
                let mut others: Vec<usize> = extras
                    .iter()
                    .filter_map(|extra| match *extra {
                        Extra::Other { group, member } if group == g => Some(member),
                        Extra::Other { .. } | Extra::Cut(_) | Extra::Inactive(_) => None,
                    })
                    .collect();
                others.sort_unstable();
                members.extend(others);
                listed += members.len();
                let ids: HashMap<usize, String> = members
                    .iter()
                    .map(|&i| (i, id(&group.members[i].key)))
                    .collect();
                let stocks = stocks_of(analysis, group.partition);
                PartitionReport {
                    stocks: stocks
                        .iter()
                        .take(MAX_STOCKS)
                        .map(|s| names.display(s))
                        .collect(),
                    other_stocks: stocks.len().saturating_sub(MAX_STOCKS),
                    loop_count: group.matching.iter().filter(|&&m| m).count(),
                    dominance: group
                        .spans
                        .iter()
                        .map(|span| DominanceSpan {
                            // A span is its saved steps, from its first to
                            // its last: one step is from a time to itself.
                            from: round(analysis.times[span.start]),
                            to: round(analysis.times[span.end - 1]),
                            leaders: span
                                .leaders
                                .iter()
                                .take(per_span)
                                .map(|&(i, s)| LoopShare {
                                    id: ids[&i].clone(),
                                    share: rounded_share(s),
                                })
                                .collect(),
                        })
                        .collect(),
                    loops: members
                        .iter()
                        .map(|&i| {
                            loop_report(group.members[i], ids[&i].clone(), names, Detail::Overview)
                        })
                        .collect(),
                }
            })
            .collect();
        let cut = analysis.cut.as_ref().map(|cut| {
            let loops: Vec<LoopReport> = extras
                .iter()
                .filter_map(|extra| match *extra {
                    Extra::Cut(i) => Some(&cut.loops[i]),
                    Extra::Other { .. } | Extra::Inactive(_) => None,
                })
                .map(|l| loop_report(l, id(&l.key), names, Detail::Elsewhere))
                .collect();
            CutReport {
                links: cut
                    .links
                    .iter()
                    .map(|(from, to)| CutLink {
                        from: names.display(from),
                        to: names.display(to),
                    })
                    .collect(),
                other_loops: cut.loops.len() - loops.len(),
                loops,
            }
        });
        let inactive = (!self.inactive.is_empty()).then(|| {
            let loops: Vec<LoopReport> = extras
                .iter()
                .filter_map(|extra| match *extra {
                    Extra::Inactive(i) => Some(self.inactive[i]),
                    Extra::Other { .. } | Extra::Cut(_) => None,
                })
                .map(|l| loop_report(l, id(&l.key), names, Detail::Elsewhere))
                .collect();
            InactiveLoops {
                other_loops: self.inactive.len() - loops.len(),
                loops,
            }
        });
        // Every matching loop not listed: in listed partitions and the
        // others. Every loop a partition lists matches.
        let omitted_partitions = self.partitions - partitions.len();
        let omitted_loops = self.found - listed;
        Body {
            partitions,
            inactive,
            omitted: (omitted_partitions > 0 || omitted_loops > 0).then_some(OmittedLoops {
                partitions: omitted_partitions,
                loops: omitted_loops,
            }),
            cut,
        }
    }
}

/// The stocks of the partition `p` indexes, none for loops in no partition.
fn stocks_of(analysis: &LoopAnalysis, p: Option<usize>) -> &[String] {
    p.and_then(|p| analysis.partitions.get(p))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// How much of a loop a report gives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Detail {
    /// Its chain if it is short, else its stocks; and its share in the run.
    Overview,
    /// Its chain however long; and its share in the run.
    Whole,
    /// As in an overview, without a share: a loop this run does not have.
    Elsewhere,
}

fn loop_report(l: &AnalyzedLoop, id: String, names: &ModelNames<'_>, detail: Detail) -> LoopReport {
    let whole = detail == Detail::Whole || l.chain.len() <= MAX_CHAIN;
    LoopReport {
        id,
        name: l.name.clone(),
        polarity: l.polarity,
        length: l.chain.len(),
        chain: whole.then(|| {
            l.chain
                .iter()
                .map(|step| ChainLink {
                    variable: names.display(&step.node),
                    polarity: step.sign.into(),
                    via: (!step.via.is_empty()).then(|| step.via.join(", ")),
                })
                .collect()
        }),
        stocks: if whole {
            vec![]
        } else {
            l.chain
                .iter()
                .filter(|step| names.is_stock(&step.node))
                .map(|step| names.display(&step.node))
                .collect()
        },
        share: match detail {
            Detail::Overview | Detail::Whole => l.share.map(rounded_share),
            Detail::Elsewhere => None,
        },
    }
}

/// What an answer says in words: why its loops come from structure, or are
/// not every loop, or lead without dominating; and what finding no loop
/// through `through` shows.
fn note(
    analysis: &LoopAnalysis,
    through: Option<&str>,
    found: usize,
    spread: &[String],
) -> Option<String> {
    const DISTURB: &str = "To see which loop dominates, disturb the model: run_experiment with a \
                           change from a time after the start (a step in an input), then analyze \
                           that run.";
    const INACTIVE: &str = "No loop was active in this run: every loop's score is zero \
                            throughout, or its stocks move only by the rounding of what they are \
                            computed from, as in a model at rest";
    let mut notes: Vec<String> = Vec::new();
    match analysis.basis {
        LoopBasis::Structure if analysis.conveyors => {
            notes.push(format!(
                "The engine does not analyze loops through a conveyor or a queue: a run of this \
                 model scores none of its loops, and its structure leaves out the links a \
                 conveyor or queue makes. {}",
                if analysis.loops.is_empty() {
                    "No loop is found without them."
                } else {
                    "These are its loops from structure that do not pass through one, with their \
                     equations' signs."
                }
            ));
        }
        LoopBasis::Structure if analysis.enumerated && analysis.loops.is_empty() => {
            notes.push(if analysis.cut.is_some() {
                "With its equations replaced, the model has no feedback loops.".to_string()
            } else {
                "The model has no feedback loops.".to_string()
            });
        }
        LoopBasis::Structure if analysis.enumerated => {
            notes.push(format!(
                "{INACTIVE}, so which loop dominates is undefined. These are the model's loops \
                 from its structure, with their equations' signs. {DISTURB}"
            ));
        }
        LoopBasis::Structure => {
            notes.push(format!(
                "{INACTIVE}, and the model has too many loops to list from its structure alone. \
                 {DISTURB}"
            ));
        }
        LoopBasis::Run => {
            if analysis.sampled {
                notes.push(
                    "The model has too many loops to enumerate, so these were found by searching \
                     the run for the strongest: a sample, not every loop."
                        .to_string(),
                );
            }
            let kept = analysis.loops.len();
            let loops = |n: usize| match n {
                1 => "1 loop".to_string(),
                n => format!("{n} loops"),
            };
            let their = |n: usize| if n == 1 { "its" } else { "their" };
            match (analysis.capped_from, analysis.negligible) {
                (None, 0) => {}
                (Some(retained), 0) => notes.push(format!(
                    "The analysis keeps the {kept} most important of the run's {retained} loops."
                )),
                (None, negligible) => notes.push(format!(
                    "The analysis leaves out {} that never held a thousandth of {} \
                     partition's activity.",
                    loops(negligible),
                    their(negligible)
                )),
                (Some(retained), negligible) => notes.push(format!(
                    "Of the run's {} loops, {negligible} never held a thousandth of {} \
                     partition's activity; the analysis keeps the {kept} most important of \
                     the other {retained}.",
                    retained + negligible,
                    their(negligible)
                )),
            }
            if !spread.is_empty() {
                notes.push(format!(
                    "In the partition{} with {}, the spans whose leader holds less than a tenth \
                     of the loop activity have it spread across many loops: no one loop \
                     dominates there.",
                    if spread.len() == 1 { "" } else { "s" },
                    spread.join(", ")
                ));
            }
        }
    }
    if let Some(through) = through
        && found == 0
        && !analysis.loops.is_empty()
    {
        notes.push(match (analysis.shows_absence(), analysis.basis) {
            (true, LoopBasis::Run) if analysis.complete() => {
                format!("No loop active in this run goes through {through}.")
            }
            // The structure's other loops are inactive in the run or among
            // those the analysis left out, which it cannot tell apart.
            (true, LoopBasis::Run) => format!(
                "No loop this analysis reports for the run goes through {through}; the model's \
                 other loops are listed as inactive."
            ),
            (true, LoopBasis::Structure) => format!("No loop goes through {through}."),
            (false, _) => format!(
                "None of the loops the analysis has goes through {through}; it does not have \
                 every loop, so one it left out may."
            ),
        });
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

#[cfg(test)]
#[path = "loops_tests.rs"]
mod tests;
