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
//! partition's stocks; a dominance timeline over about a dozen windows of the
//! run, adjacent windows merged while the same loop leads, each naming the
//! strongest loop there and its rivals, with their shares of the partition's
//! loop activity; and the partition's loops, every one the timeline names and
//! the most important others. A loop is reported with its session id, its
//! polarity as the run shows it, its chain from a stock around to the start
//! with each link's sign (a builtin's or macro's internal nodes left out, the
//! links across them composed), and its mean share. The answer keeps to the
//! outline's budget by listing fewer loops: a partition's first few and every
//! partition's leaders before any partition's others.
//!
//! Loop ids (`L1`, `L2`, ...) are the session's, keyed by the loop's cycle --
//! its node sequence, rotation-invariant and direction-preserving -- so a loop
//! keeps its id across runs and edits for as long as it exists, whatever the
//! engine's own loop ids are.
//!
//! A link's sign is the one the run gives it when the link was active with one
//! sign throughout (the classification a loop's polarity uses), and its
//! equation's sign otherwise.
//!
//! A run in which no loop is active -- a model at rest, whose loop scores are
//! zero throughout -- has no dominance and no runtime polarity to report. The
//! answer says so, lists the model's loops from its structure with their
//! equations' signs, and names the repair: an experiment that disturbs the
//! model.
//!
//! A run whose plan replaces equations reports what they cut: the links each
//! replaced variable read and no longer reads, and the model's loops through
//! them (its structural loops when the structure alone enumerates them, else
//! the loops of its current run).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, Variable};
use crate::db::{
    DetectedLoopPolarity, LtmMode, LtmOverlay, SimlinDb, SourceModel, SourceProject,
    set_project_ltm_discovery_mode,
};
use crate::ltm::{
    LinkPolarity, LoopPolarity, canonical_rotation, is_synthetic_node_name, strip_subscript,
};
use crate::results::Results;

use super::evidence::Evidence;
use super::runs::{self, CURRENT, Run, RunPlan, RunStore};
use super::series::round;
use super::variables::LinkPolarityName;
use super::{Session, ToolError, Workspace, names, resolve_model};

/// How long discovery may search a run for loops before it reports what it
/// found.
const DISCOVERY_BUDGET: Duration = Duration::from_secs(20);

/// The windows a dominance timeline divides a run into.
pub(crate) const WINDOWS: usize = 12;

/// The share of a partition's loop activity a loop holds, on average over a
/// window, to be active there: discovery's own floor for a loop worth
/// reporting.
pub(crate) const ACTIVE_SHARE: f64 = 0.001;

/// The share of the strongest loop's a loop holds in a window to lead beside
/// it.
pub(crate) const RIVAL_SHARE: f64 = 0.5;

/// The most leaders a timeline span names.
pub(crate) const MAX_LEADERS: usize = 3;

/// The share below which a partition's strongest loop does not dominate it:
/// its activity is spread across many loops.
pub(crate) const DOMINANT_SHARE: f64 = 0.1;

/// The loops a partition lists beyond those its timeline names.
pub(crate) const MAX_LOOPS: usize = 8;

/// The loops beyond its leaders a partition keeps listed while others are
/// left out for the budget.
const MIN_LOOPS: usize = 3;

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

impl From<DetectedLoopPolarity> for LoopPolarityName {
    fn from(polarity: DetectedLoopPolarity) -> LoopPolarityName {
        match polarity {
            DetectedLoopPolarity::Reinforcing => LoopPolarityName::Reinforcing,
            DetectedLoopPolarity::Balancing => LoopPolarityName::Balancing,
            DetectedLoopPolarity::MostlyReinforcing => LoopPolarityName::MostlyReinforcing,
            DetectedLoopPolarity::MostlyBalancing => LoopPolarityName::MostlyBalancing,
            DetectedLoopPolarity::Undetermined => LoopPolarityName::Undetermined,
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
    /// The loops the analysis found (those through `through`, when given).
    pub found: usize,
    /// Whether the loops were drawn from every loop there is: false when the
    /// model has too many to enumerate, so the run's were found by searching
    /// for the strongest (a sample), or too many for its structure alone to
    /// list.
    pub complete: bool,
    /// The partitions the loops are in, largest first.
    pub partitions: Vec<PartitionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<OmittedLoops>,
    /// Loops asked for by id that this run does not have: inactive in it, or
    /// cut from it.
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
    /// most 12. Empty for loops inside a module, which belong to no
    /// partition of this model.
    pub stocks: Vec<String>,
    /// How many more stocks it has than it names.
    #[serde(skip_serializing_if = "is_zero")]
    pub other_stocks: usize,
    /// How many of its loops the analysis found (through `through`, when
    /// given).
    pub loop_count: usize,
    /// Which loops led, span by span: from the run's start to its end in at
    /// most 12 spans. Absent for loops from structure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dominance: Vec<DominanceSpan>,
    /// Every loop `dominance` names, then the most important others (the
    /// shortest, from structure); at most 8 besides the leaders.
    pub loops: Vec<LoopReport>,
}

/// A span of a run and the loops that led in it: the strongest on average
/// over the span, and those holding at least half its share, at most 3. None
/// led a span in which no loop was active.
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
    /// Its share of the partition's loop activity, from 0 to 1: at each time
    /// the shares of all the partition's loops sum to 1, so where many loops
    /// are active each holds little.
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
    /// Its share of the partition's loop activity, averaged over the run,
    /// from 0 to 1. Absent for loops from structure.
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
}

/// Loops the analysis found and the answer does not list.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct OmittedLoops {
    /// Partitions past the 6 listed.
    pub partitions: usize,
    /// Loops not listed, in listed partitions and the others. `through` finds
    /// the loops through a variable.
    pub loops: usize,
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

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A run's loops as an analysis keeps them, before the session names them.
pub(crate) struct LoopAnalysis {
    basis: LoopBasis,
    times: Vec<f64>,
    loops: Vec<AnalyzedLoop>,
    /// Each partition's stocks (canonical, element-level), indexed by
    /// [`AnalyzedLoop::partition`].
    partitions: Vec<Vec<String>>,
    complete: bool,
    /// How many loops discovery kept before its cap, when the cap bound.
    capped_from: Option<usize>,
    /// Whether the model has a conveyor or a queue: a run of it scores no
    /// loop, and its structure leaves out the links they make.
    conveyors: bool,
    /// Whether the run's stocks do not move, which is why an analysis from
    /// structure has no run's loops.
    at_rest: bool,
    cut: Option<Cut>,
}

impl LoopAnalysis {
    /// About how many bytes the analysis holds: its times, and each loop's
    /// score series and names. What a run store counts a kept analysis as.
    pub(crate) fn bytes(&self) -> usize {
        let text = |names: &[String]| names.iter().map(String::len).sum::<usize>();
        let numbers = self.times.len() + self.loops.iter().map(|l| l.rel.len()).sum::<usize>();
        numbers * std::mem::size_of::<f64>()
            + self
                .loops
                .iter()
                .map(|l| text(&l.key) + text(&l.chain))
                .sum::<usize>()
            + self.partitions.iter().map(|p| text(p)).sum::<usize>()
    }
}

/// One loop of an analysis.
#[derive(Clone)]
pub(crate) struct AnalyzedLoop {
    /// The cycle's canonical rotation: what identifies the loop.
    key: Vec<String>,
    /// The cycle from a stock (canonical names).
    chain: Vec<String>,
    /// `signs[i]` is the link from `chain[i]` to the next node.
    signs: Vec<LinkPolarity>,
    polarity: LoopPolarityName,
    /// The signed partition-relative score at each saved step; empty from
    /// structure.
    rel: Vec<f64>,
    /// The mean of `|rel|`; `None` from structure.
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
                self.chain.contains(&node)
            }
            None => self.chain.iter().any(|node| strip_subscript(node) == ident),
        }
    }

    /// Whether the loop has the link from `from` to `to` (canonical variable
    /// idents), at any element.
    fn has_link(&self, from: &str, to: &str) -> bool {
        let n = self.chain.len();
        (0..n).any(|i| {
            strip_subscript(&self.chain[i]) == from
                && strip_subscript(&self.chain[(i + 1) % n]) == to
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
        ToolError::new(format!("the model has no variable '{name}'")).with_suggestions(suggestions)
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
    let mut left_out = Vec::new();
    loop {
        let mut answer = report_by_id_of(analysis, model, run, revision, &asked);
        answer.left_out = left_out.clone();
        let size = serde_json::to_string(&answer).map_or(0, |json| json.len());
        if size <= budget || asked.len() <= 1 {
            return answer;
        }
        let (id, _) = asked.pop().expect("more than one");
        left_out.insert(0, id);
    }
}

fn report_by_id_of(
    analysis: &LoopAnalysis,
    model: &datamodel::Model,
    run: &str,
    revision: u64,
    keys: &[(String, Vec<String>)],
) -> AnalyzeLoopsOutput {
    let names = ModelNames::new(model);
    let mut groups: Vec<(Option<usize>, Vec<LoopReport>)> = Vec::new();
    let mut absent = Vec::new();
    for (id, key) in keys {
        let Some(l) = analysis.loops.iter().find(|l| l.key == *key) else {
            absent.push(id.clone());
            continue;
        };
        let report = loop_report(l, id.clone(), &names, Detail::Whole);
        match groups.iter_mut().find(|(p, _)| *p == l.partition) {
            Some((_, reports)) => reports.push(report),
            None => groups.push((l.partition, vec![report])),
        }
    }
    let found = groups.iter().map(|(_, reports)| reports.len()).sum();
    let partitions = groups
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
            "{} {} not a loop of this run: inactive in it, or cut from it.",
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
        complete: analysis.complete,
        partitions,
        omitted: None,
        absent,
        left_out: vec![],
        cut: None,
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
/// first, at most `n`: the leaders of every partition's spans that end after
/// it, each by its largest share in them. None from structure.
pub(crate) fn leaders_after(
    evidence: &mut Evidence,
    analysis: &LoopAnalysis,
    time: f64,
    n: usize,
) -> Vec<String> {
    if analysis.basis != LoopBasis::Run {
        return vec![];
    }
    let mut partitions: Vec<(Option<usize>, Vec<&AnalyzedLoop>)> = Vec::new();
    for l in &analysis.loops {
        match partitions.iter_mut().find(|(p, _)| *p == l.partition) {
            Some((_, members)) => members.push(l),
            None => partitions.push((l.partition, vec![l])),
        }
    }
    let mut leaders: Vec<(&AnalyzedLoop, f64)> = Vec::new();
    for (_, members) in &partitions {
        for span in dominance(members, analysis.times.len()) {
            let end = analysis
                .times
                .get(span.end)
                .or(analysis.times.last())
                .copied()
                .unwrap_or(f64::NEG_INFINITY);
            if end <= time {
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

/// How often a loop led its partition over a span of a run.
pub(crate) struct Leadership {
    /// The steps it was the strongest in.
    pub led: usize,
    /// The steps some loop of the partition was active in.
    pub active: usize,
    /// The cycle of the loop that was the strongest in the most steps.
    pub most: Option<Vec<String>>,
}

/// How often the loop whose cycle is `key` was its partition's strongest
/// over the saved steps from `from` to `to`, counting the steps in which some
/// loop of the partition was active; `None` when the analysis does not have
/// the loop. A run with no active loop (loops from structure) has none
/// active.
pub(crate) fn leadership(
    analysis: &LoopAnalysis,
    key: &[String],
    from: f64,
    to: f64,
) -> Option<Leadership> {
    let target = analysis.loops.iter().find(|l| l.key == key)?;
    let peers: Vec<&AnalyzedLoop> = analysis
        .loops
        .iter()
        .filter(|l| l.partition == target.partition)
        .collect();
    let mut counts: Vec<usize> = vec![0; peers.len()];
    let mut active = 0;
    for (step, &time) in analysis.times.iter().enumerate() {
        if time < from || time > to {
            continue;
        }
        let strongest = peers
            .iter()
            .enumerate()
            .map(|(i, l)| (i, l.rel.get(step).map_or(0.0, |s| s.abs())))
            .fold(
                (0, 0.0_f64),
                |best, (i, s)| if s > best.1 { (i, s) } else { best },
            );
        if strongest.1 >= ACTIVE_SHARE {
            active += 1;
            counts[strongest.0] += 1;
        }
    }
    let target_index = peers.iter().position(|l| l.key == key).expect("a peer");
    let most = counts
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
        .filter(|&(_, &n)| n > 0)
        .map(|(i, _)| peers[i].key.clone());
    Some(Leadership {
        led: counts[target_index],
        active,
        most,
    })
}

/// The cycles of the analysis's loops through the variable `ident`
/// (canonical); `None` when the analysis is not of every loop, so that no
/// absence can be shown.
pub(crate) fn loops_through(analysis: &LoopAnalysis, ident: &str) -> Option<Vec<Vec<String>>> {
    analysis.complete.then(|| {
        analysis
            .loops
            .iter()
            .filter(|l| l.goes_through(ident))
            .map(|l| l.key.clone())
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
/// in discovery mode, from its `results`; the model's loops from structure
/// when none was active. Stops before discovery, and before the loops from structure, when
/// `waiting` says other work waits for the project.
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

    let ltm_vars = crate::db::model_ltm_variables(db, source_model, source_project);
    let dims = crate::db::project_datamodel_dims(db, source_project);
    let expansion = crate::analysis::build_link_expansion_context(db, source_model, source_project);
    let offsets: HashMap<(String, String), usize> =
        crate::ltm_finding::link_score_offsets(results, &ltm_vars.vars, dims, &expansion)
            .into_iter()
            .map(|((from, to), offset)| {
                ((from.as_str().to_string(), to.as_str().to_string()), offset)
            })
            .collect();
    let scores = LinkScores::new(&offsets);

    let names = ModelNames::new(model);
    let loop_names = LoopNames::new(model, project);
    let mut seen = HashSet::new();
    let loops: Vec<AnalyzedLoop> = discovery
        .loops
        .iter()
        .filter_map(|found| {
            let equation_sign: HashMap<(&str, &str), LinkPolarity> = found
                .loop_info
                .links
                .iter()
                .map(|link| ((link.from.as_str(), link.to.as_str()), link.polarity))
                .collect();
            let (key, chain, signs) = names.cycle(&node_sequence(&found.loop_info), |from, to| {
                scores.sign(results, from, to).unwrap_or_else(|| {
                    equation_sign
                        .get(&(from, to))
                        .copied()
                        .unwrap_or(LinkPolarity::Unknown)
                })
            });
            if !seen.insert(key.clone()) {
                return None;
            }
            let rel: Vec<f64> = found
                .rel_scores
                .iter()
                .map(|s| if s.is_finite() { *s } else { 0.0 })
                .collect();
            let share = mean_abs(&rel);
            Some(AnalyzedLoop {
                name: loop_names.of(&chain),
                key,
                chain,
                signs,
                polarity: found.loop_info.polarity.clone().into(),
                rel,
                share: Some(share),
                partition: found.partition,
            })
        })
        .collect();

    // A run whose stocks do not move, to the precision a summary reports
    // them, is at rest however its loop scores round: dominance there is
    // arithmetic noise, so its loops come from structure.
    let at_rest = stocks_at_rest(model, plan, results);
    if !loops.is_empty() && !at_rest {
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
            complete: discovery.enumeration_complete
                && !discovery.agg_recovery_truncated
                && !discovery.truncated,
            capped_from: (discovery.retained_loops > discovery.loops.len())
                .then_some(discovery.retained_loops),
            conveyors: false,
            at_rest: false,
            cut: None,
        });
    }

    // A conveyor or queue builds through a path that synthesizes no loop
    // scores, and the causal graph has no link through one.
    let conveyors = crate::conveyor_compile::project_has_conveyor(project, &model.name)
        || crate::queue_compile::project_has_queue(project, &model.name);
    yield_point()?;
    let structure = with_discovery_mode(db, source_project, false, |db| {
        structural_loops(db, source_project, source_model, model, project)
    });
    let complete = structure.is_some() && !conveyors;
    let structure = structure.unwrap_or_default();
    Ok(LoopAnalysis {
        basis: LoopBasis::Structure,
        times: vec![],
        loops: structure.loops,
        partitions: structure.partitions,
        complete,
        capped_from: None,
        conveyors,
        at_rest,
        cut: None,
    })
}

/// Whether every stock of `model` is at rest in `results`, a run under
/// `plan`, each element of an arrayed one: what a behavior summary would call
/// it. False for a model with no stock.
fn stocks_at_rest(model: &datamodel::Model, plan: &RunPlan, results: &Results) -> bool {
    let times: Vec<f64> = results
        .iter()
        .map(|row| row[crate::results::TIME_OFF])
        .collect();
    let mut any = false;
    for var in &model.variables {
        let Variable::Stock(stock) = var else {
            continue;
        };
        let canonical = crate::canonicalize(&stock.ident).into_owned();
        let prefix = format!("{canonical}[");
        for (key, &offset) in &results.offsets {
            if key.as_str() != canonical && !key.as_str().starts_with(&prefix) {
                continue;
            }
            any = true;
            let series: Vec<f64> = results.iter().map(|row| row[offset]).collect();
            let scale = super::series::scale_in_run(results, model, plan, key.as_str());
            if super::behavior::classify_at(&times, &series, scale).kind
                != super::behavior::ModeKind::AtRest
            {
                return false;
            }
        }
    }
    any
}

/// The link scores of a run under the LTM overlay, by `(from, to)` element
/// link, to sign a discovered loop's links with.
struct LinkScores<'a> {
    offsets: &'a HashMap<(String, String), usize>,
    /// The links out of each aggregate node, the synthetic node the engine
    /// routes an array reducer (`SUM(pop[*])`) through.
    out_of_aggregate: HashMap<&'a str, Vec<(&'a str, usize)>>,
}

impl<'a> LinkScores<'a> {
    /// The longest chain of aggregate nodes a link is signed across: a
    /// reducer of a reducer's result is two.
    const MAX_AGGREGATES: usize = 4;

    fn new(offsets: &'a HashMap<(String, String), usize>) -> LinkScores<'a> {
        let mut out_of_aggregate: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
        for ((from, to), &offset) in offsets {
            if is_aggregate(from) {
                out_of_aggregate
                    .entry(from.as_str())
                    .or_default()
                    .push((to.as_str(), offset));
            }
        }
        LinkScores {
            offsets,
            out_of_aggregate,
        }
    }

    /// The sign of the link `from -> to` in the run: its link score's, or,
    /// for a link discovery stitched across aggregate nodes, the sign of the
    /// path score through them (the product of the link scores along the
    /// path) when every such path has the same one.
    ///
    /// `Unknown` when the link was active and its sign changed over the run,
    /// as a loop's polarity is undetermined, so a loop the run leaves
    /// undetermined shows which of its links did. `None` when the run never
    /// scored the link, and its equation's sign stands.
    fn sign(&self, results: &Results, from: &str, to: &str) -> Option<LinkPolarity> {
        let series_of = |path: &[usize]| -> Vec<f64> {
            results
                .iter()
                .map(|row| path.iter().map(|&offset| row[offset]).product())
                .collect()
        };
        let paths = match self.offsets.get(&(from.to_string(), to.to_string())) {
            Some(&offset) => vec![vec![offset]],
            None => self.paths(from, to),
        };
        let mut signs = paths
            .iter()
            .filter_map(|path| runtime_sign(&series_of(path)));
        let first = signs.next()?;
        Some(if signs.all(|sign| sign == first) {
            first
        } else {
            LinkPolarity::Unknown
        })
    }

    /// Each path of link-score offsets from `from` to `to` through aggregate
    /// nodes only.
    fn paths(&self, from: &str, to: &str) -> Vec<Vec<usize>> {
        let mut found = Vec::new();
        for ((source, node), &offset) in self.offsets {
            if source == from && is_aggregate(node) {
                self.extend(node, to, vec![offset], &mut found);
            }
        }
        found
    }

    fn extend(&self, node: &str, to: &str, path: Vec<usize>, found: &mut Vec<Vec<usize>>) {
        for &(next, offset) in self.out_of_aggregate.get(node).into_iter().flatten() {
            let mut longer = path.clone();
            longer.push(offset);
            if next == to {
                found.push(longer);
            } else if is_aggregate(next) && longer.len() <= Self::MAX_AGGREGATES {
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

/// A link's sign from its score over a run, by the rule a loop's polarity is
/// read by ([`LoopPolarity::from_runtime_scores`]): `Unknown` when its sign
/// changed and its scores net to less than 99% of their magnitude, `None`
/// when it was never active.
fn runtime_sign(series: &[f64]) -> Option<LinkPolarity> {
    Some(match LoopPolarity::from_runtime_scores(series)?.0 {
        LoopPolarity::Reinforcing | LoopPolarity::MostlyReinforcing => LinkPolarity::Positive,
        LoopPolarity::Balancing | LoopPolarity::MostlyBalancing => LinkPolarity::Negative,
        LoopPolarity::Undetermined => LinkPolarity::Unknown,
    })
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
    let polarities = crate::db::compute_link_polarities(db, source_model, source_project);
    let names = ModelNames::new(model);
    let loop_names = LoopNames::new(model, project);
    let mut seen = HashSet::new();
    let loops = detected
        .loops
        .iter()
        .filter_map(|detected| {
            let (key, chain, signs) = names.cycle(&detected.variables, |from, to| {
                polarities
                    .get(&(
                        strip_subscript(from).to_string(),
                        strip_subscript(to).to_string(),
                    ))
                    .copied()
                    .unwrap_or(LinkPolarity::Unknown)
            });
            if !seen.insert(key.clone()) {
                return None;
            }
            Some(AnalyzedLoop {
                name: detected.name.clone().or_else(|| loop_names.of(&chain)),
                key,
                chain,
                signs,
                polarity: detected.polarity.into(),
                rel: vec![],
                share: None,
                partition: detected.partition,
            })
        })
        .collect();
    Some(Structure {
        loops,
        partitions: detected
            .partitions
            .iter()
            .map(|p| p.stocks.clone())
            .collect(),
    })
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

/// A loop's nodes in its links' order. Discovery has already collapsed its
/// aggregate nodes, so a link across one is signed by [`LinkScores::sign`].
fn node_sequence(l: &crate::ltm::Loop) -> Vec<String> {
    let Some(last) = l.links.last() else {
        return Vec::new();
    };
    std::iter::once(last)
        .chain(l.links.iter().take(l.links.len() - 1))
        .map(|link| link.to.to_string())
        .collect()
}

/// A cycle and its links' signs with the engine's synthetic nodes (a
/// builtin's or macro's internals, an aggregate) left out, each link across
/// them signed by composing the links it stands for, from the first node that
/// is not one.
fn without_synthetic(
    chain: Vec<String>,
    signs: Vec<LinkPolarity>,
) -> (Vec<String>, Vec<LinkPolarity>) {
    let Some(start) = chain.iter().position(|node| !is_synthetic_node_name(node)) else {
        return (chain, signs);
    };
    let n = chain.len();
    let mut nodes: Vec<String> = Vec::with_capacity(n);
    let mut composed: Vec<LinkPolarity> = Vec::with_capacity(n);
    for i in (start..n).chain(0..start) {
        if is_synthetic_node_name(&chain[i]) {
            let last = composed.last_mut().expect("the walk starts at a real node");
            *last = last.compose(signs[i]);
        } else {
            nodes.push(chain[i].clone());
            composed.push(signs[i]);
        }
    }
    (nodes, composed)
}

/// The links of a cycle, `(chain[i], chain[i + 1])`, the last returning to the
/// first.
fn links_of(chain: &[String]) -> impl Iterator<Item = (&str, &str)> {
    let n = chain.len();
    (0..n).map(move |i| (chain[i].as_str(), chain[(i + 1) % n].as_str()))
}

fn mean_abs(series: &[f64]) -> f64 {
    if series.is_empty() {
        return 0.0;
    }
    series.iter().map(|s| s.abs()).sum::<f64>() / series.len() as f64
}

/// A share rounded to hundredths.
fn rounded_share(x: f64) -> f64 {
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

    /// The loop through `nodes` (its node sequence, as the engine reports
    /// it) as a report gives it: its key, its chain from a stock with the
    /// engine's synthetic nodes left out, and each link's sign, from
    /// `sign(from, to)` for the links of `nodes`.
    ///
    /// The key is the chain's canonical rotation, taken after the synthetic
    /// nodes are left out, so a loop is known by the variables a person sees:
    /// the same from a run and from structure, however each spells a
    /// builtin's internals.
    fn cycle(
        &self,
        nodes: &[String],
        sign: impl Fn(&str, &str) -> LinkPolarity,
    ) -> (Vec<String>, Vec<String>, Vec<LinkPolarity>) {
        let start = nodes
            .iter()
            .position(|node| self.is_stock(node))
            .unwrap_or(0);
        let from_a_stock: Vec<String> = nodes[start..]
            .iter()
            .chain(&nodes[..start])
            .cloned()
            .collect();
        let signs = links_of(&from_a_stock)
            .map(|(from, to)| sign(from, to))
            .collect();
        let (chain, signs) = without_synthetic(from_a_stock, signs);
        (canonical_rotation(&chain), chain, signs)
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
    fn of(&self, chain: &[String]) -> Option<String> {
        let variables: HashSet<String> = chain
            .iter()
            .map(|node| strip_subscript(node).to_string())
            .collect();
        crate::analysis::persisted_loop_name(
            &variables,
            &self.by_uids,
            &self.model.name,
            self.project,
        )
    }
}

/// A timeline span over saved steps `[start, end)`, and its leaders as
/// indices into the partition's loops with their shares.
struct Span {
    start: usize,
    end: usize,
    leaders: Vec<(usize, f64)>,
}

/// The dominance timeline of a partition's loops over a run of `steps` saved
/// steps: [`WINDOWS`] windows, adjacent ones merged while the same loop leads,
/// and each boundary between two leaders moved to the step at which the new
/// one overtakes the old.
fn dominance(loops: &[&AnalyzedLoop], steps: usize) -> Vec<Span> {
    let leaders_over = |start: usize, end: usize| {
        let mut shares: Vec<(usize, f64)> = loops
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let window = &l.rel[start.min(l.rel.len())..end.min(l.rel.len())];
                let total: f64 = window.iter().map(|s| s.abs()).sum();
                (i, total / (end - start).max(1) as f64)
            })
            .filter(|&(_, share)| share >= ACTIVE_SHARE)
            .collect();
        shares.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let strongest = shares.first().map_or(0.0, |&(_, share)| share);
        shares.retain(|&(_, share)| share >= RIVAL_SHARE * strongest);
        shares.truncate(MAX_LEADERS);
        shares
    };
    let windows = WINDOWS.min(steps);
    // Each merged span: its steps, the loop that leads each of its windows
    // (`None` where none does), and where its first window ends and its last
    // begins.
    struct Merged {
        start: usize,
        end: usize,
        top: Option<usize>,
        first_window_end: usize,
        last_window_start: usize,
    }
    let mut merged: Vec<Merged> = Vec::new();
    for w in 0..windows {
        let (start, end) = (w * steps / windows, (w + 1) * steps / windows);
        let top = leaders_over(start, end).first().map(|&(i, _)| i);
        match merged.last_mut() {
            Some(last) if last.top == top => {
                last.end = end;
                last.last_window_start = start;
            }
            _ => merged.push(Merged {
                start,
                end,
                top,
                first_window_end: end,
                last_window_start: start,
            }),
        }
    }
    // Between two spans each led by a loop, the boundary is the step from
    // which the second loop stays at least as strong as the first through the
    // windows either side, so a switch mid-window reads where it happened
    // rather than at the window's edge.
    let magnitude = |i: usize, k: usize| loops[i].rel.get(k).map_or(0.0, |s| s.abs());
    for i in 1..merged.len() {
        let (Some(old), Some(new)) = (merged[i - 1].top, merged[i].top) else {
            continue;
        };
        let (from, to) = (merged[i - 1].last_window_start, merged[i].first_window_end);
        let mut boundary = to;
        while boundary > from && magnitude(new, boundary - 1) >= magnitude(old, boundary - 1) {
            boundary -= 1;
        }
        if boundary > merged[i - 1].start && boundary < merged[i].end && boundary < to {
            merged[i - 1].end = boundary;
            merged[i].start = boundary;
        }
    }
    merged
        .into_iter()
        .map(|span| Span {
            start: span.start,
            end: span.end,
            leaders: leaders_over(span.start, span.end),
        })
        .collect()
}

/// The answer for `analysis`, its loops named by `evidence`, within `budget`
/// bytes: what it lists is left out, least important first
/// ([`Selection::shed`]), until it fits.
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
    let mut selection = Selection::new(analysis, through_ident.as_deref());
    let answer = |selection: &Selection<'_>, evidence: &mut Evidence| {
        let body = selection.build(evidence, analysis, &names);
        AnalyzeLoopsOutput {
            revision,
            run: run.to_string(),
            basis: analysis.basis,
            through: through.clone(),
            found: selection.found,
            complete: analysis.complete,
            partitions: body.partitions,
            omitted: body.omitted,
            absent: vec![],
            left_out: vec![],
            cut: body.cut,
            note: note(
                analysis,
                through.as_deref(),
                selection.found,
                &selection.spread(analysis, &names),
            ),
        }
    };
    // Fit on a copy of the session's ids, so a loop left out is given none.
    loop {
        let trial = answer(&selection, &mut evidence.clone());
        let size = serde_json::to_string(&trial).map_or(0, |json| json.len());
        if size <= budget || !selection.shed() {
            break;
        }
    }
    answer(&selection, evidence)
}

/// The partitions, loops and cut an answer lists, before the session names
/// them.
struct Selection<'a> {
    groups: Vec<Group<'a>>,
    /// The loops the analysis found that match.
    found: usize,
    omitted_partitions: usize,
    /// How many of the cut's loops are listed.
    cut_listed: usize,
}

/// A partition an answer lists.
struct Group<'a> {
    partition: Option<usize>,
    /// Its loops, most important first.
    members: Vec<&'a AnalyzedLoop>,
    spans: Vec<Span>,
    /// Indices into `members`: its leaders, then the others it lists.
    listed: Vec<usize>,
    leaders: usize,
    /// Whether each of `members` matches.
    matching: Vec<bool>,
}

impl Group<'_> {
    fn matching_count(&self) -> usize {
        self.matching.iter().filter(|&&m| m).count()
    }
}

/// What [`Selection::build`] makes: the answer's parts that depend on the
/// selection.
struct Body {
    partitions: Vec<PartitionReport>,
    omitted: Option<OmittedLoops>,
    cut: Option<CutReport>,
}

impl<'a> Selection<'a> {
    fn new(analysis: &'a LoopAnalysis, through: Option<&str>) -> Selection<'a> {
        let matches = |l: &AnalyzedLoop| through.is_none_or(|ident| l.goes_through(ident));

        // The loops of each partition; loops in no partition last.
        let mut groups: Vec<(Option<usize>, Vec<&AnalyzedLoop>)> = Vec::new();
        for l in &analysis.loops {
            match groups.iter_mut().find(|(p, _)| *p == l.partition) {
                Some((_, members)) => members.push(l),
                None => groups.push((l.partition, vec![l])),
            }
        }
        groups.retain(|(_, members)| members.iter().any(|l| matches(l)));
        groups.sort_by(|(a, _), (b, _)| {
            let (a_stocks, b_stocks) = (stocks_of(analysis, *a), stocks_of(analysis, *b));
            a.is_none()
                .cmp(&b.is_none())
                .then(b_stocks.len().cmp(&a_stocks.len()))
                .then(a_stocks.cmp(b_stocks))
        });

        let found = analysis.loops.iter().filter(|l| matches(l)).count();
        let omitted_partitions = groups.len().saturating_sub(MAX_PARTITIONS);
        let groups = groups
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
                let spans = match analysis.basis {
                    LoopBasis::Run => dominance(&members, analysis.times.len()),
                    LoopBasis::Structure => vec![],
                };
                let mut listed: Vec<usize> = spans
                    .iter()
                    .flat_map(|span| span.leaders.iter().map(|&(i, _)| i))
                    .collect::<HashSet<usize>>()
                    .into_iter()
                    .collect();
                listed.sort_unstable();
                let leaders = listed.len();
                for (i, l) in members.iter().enumerate() {
                    if listed.len() - leaders >= MAX_LOOPS {
                        break;
                    }
                    if !listed.contains(&i) && matches(l) {
                        listed.push(i);
                    }
                }
                let matching = members.iter().map(|l| matches(l)).collect();
                Group {
                    partition,
                    members,
                    spans,
                    listed,
                    leaders,
                    matching,
                }
            })
            .collect();
        Selection {
            groups,
            found,
            omitted_partitions,
            cut_listed: analysis
                .cut
                .as_ref()
                .map_or(0, |cut| cut.loops.len().min(MAX_LOOPS)),
        }
    }

    /// The first stock of each listed partition in which, over some span,
    /// the strongest loop does not dominate.
    fn spread(&self, analysis: &LoopAnalysis, names: &ModelNames<'_>) -> Vec<String> {
        self.groups
            .iter()
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

    /// Leave the least important thing listed out; false when nothing is left
    /// to. In order: a partition's loops past its leaders and first
    /// [`MIN_LOOPS`] others, the smallest partition's first; the cut's loops
    /// past its first; the smallest partitions, past the first; the first
    /// partition's other loops; the timeline's rivals of each span's
    /// strongest loop.
    fn shed(&mut self) -> bool {
        let others = |g: &Group<'_>| g.listed.len() - g.leaders;
        if let Some(group) = self.groups.iter_mut().rev().find(|g| others(g) > MIN_LOOPS) {
            group.listed.pop();
            return true;
        }
        if self.cut_listed > 1 {
            self.cut_listed -= 1;
            return true;
        }
        if self.groups.len() > 1 {
            self.groups.pop();
            self.omitted_partitions += 1;
            return true;
        }
        if let Some(group) = self.groups.first_mut().filter(|g| others(g) > 0) {
            group.listed.pop();
            return true;
        }
        // Last, the timeline's rivals, the last partition's first: a loop no
        // span names any more is no longer listed.
        for group in self.groups.iter_mut().rev() {
            let Some(span) = group
                .spans
                .iter_mut()
                .filter(|span| span.leaders.len() > 1)
                .max_by_key(|span| span.leaders.len())
            else {
                continue;
            };
            let (rival, _) = span.leaders.pop().expect("more than one");
            let named = group
                .spans
                .iter()
                .any(|span| span.leaders.iter().any(|&(i, _)| i == rival));
            if !named {
                group.listed.retain(|&i| i != rival);
                group.leaders -= 1;
            }
            return true;
        }
        false
    }

    /// The answer's parts, the loops they list named by `evidence`.
    fn build(
        &self,
        evidence: &mut Evidence,
        analysis: &LoopAnalysis,
        names: &ModelNames<'_>,
    ) -> Body {
        let mut listed_matching = 0;
        let partitions = self
            .groups
            .iter()
            .map(|group| {
                let ids: HashMap<usize, String> = group
                    .listed
                    .iter()
                    .map(|&i| (i, evidence.loop_id(&group.members[i].key)))
                    .collect();
                // Leaders are listed whether or not they match.
                listed_matching += group.listed.iter().filter(|&&i| group.matching[i]).count();
                let stocks = stocks_of(analysis, group.partition);
                PartitionReport {
                    stocks: stocks
                        .iter()
                        .take(MAX_STOCKS)
                        .map(|s| names.display(s))
                        .collect(),
                    other_stocks: stocks.len().saturating_sub(MAX_STOCKS),
                    loop_count: group.matching_count(),
                    dominance: group
                        .spans
                        .iter()
                        .map(|span| DominanceSpan {
                            // Spans tile the run: each ends where the next
                            // begins, and the last at the run's end.
                            from: round(analysis.times[span.start]),
                            to: round(
                                analysis
                                    .times
                                    .get(span.end)
                                    .or(analysis.times.last())
                                    .copied()
                                    .unwrap_or(0.0),
                            ),
                            leaders: span
                                .leaders
                                .iter()
                                .map(|&(i, s)| LoopShare {
                                    id: ids[&i].clone(),
                                    share: rounded_share(s),
                                })
                                .collect(),
                        })
                        .collect(),
                    loops: group
                        .listed
                        .iter()
                        .map(|&i| {
                            loop_report(group.members[i], ids[&i].clone(), names, Detail::Overview)
                        })
                        .collect(),
                }
            })
            .collect();
        let cut = analysis.cut.as_ref().map(|cut| CutReport {
            links: cut
                .links
                .iter()
                .map(|(from, to)| CutLink {
                    from: names.display(from),
                    to: names.display(to),
                })
                .collect(),
            loops: cut
                .loops
                .iter()
                .take(self.cut_listed)
                .map(|l| loop_report(l, evidence.loop_id(&l.key), names, Detail::Elsewhere))
                .collect(),
            other_loops: cut.loops.len() - self.cut_listed,
        });
        // Every matching loop not listed: in listed partitions and the others.
        let omitted_loops = self.found - listed_matching;
        Body {
            partitions,
            omitted: (self.omitted_partitions > 0 || omitted_loops > 0).then_some(OmittedLoops {
                partitions: self.omitted_partitions,
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
    /// As in an overview, without a share: a loop of another run.
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
                .zip(&l.signs)
                .map(|(node, sign)| ChainLink {
                    variable: names.display(node),
                    polarity: (*sign).into(),
                })
                .collect()
        }),
        stocks: if whole {
            vec![]
        } else {
            l.chain
                .iter()
                .filter(|node| names.is_stock(node))
                .map(|node| names.display(node))
                .collect()
        },
        share: match detail {
            Detail::Overview | Detail::Whole => l.share.map(rounded_share),
            Detail::Elsewhere => None,
        },
    }
}

/// What an answer says in words: why its loops come from structure, or are a
/// sample.
/// Why a run has no active loop.
fn inactive(analysis: &LoopAnalysis) -> &'static str {
    if analysis.at_rest {
        "its stocks do not move, as in a model at rest"
    } else {
        "every loop score is zero"
    }
}

fn note(
    analysis: &LoopAnalysis,
    through: Option<&str>,
    found: usize,
    spread: &[String],
) -> Option<String> {
    const DISTURB: &str = "To see which loop dominates, disturb the model: run_experiment with a \
                           change from a time after the start (a step in an input), then analyze \
                           that run.";
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
        LoopBasis::Structure if analysis.complete && analysis.loops.is_empty() => {
            notes.push(if analysis.cut.is_some() {
                "With its equations replaced, the model has no feedback loops.".to_string()
            } else {
                "The model has no feedback loops.".to_string()
            });
        }
        LoopBasis::Structure if analysis.complete => {
            notes.push(format!(
                "No loop was active in this run: {}, so which loop dominates is undefined. \
                 These are the model's loops from its structure, with their equations' signs. \
                 {DISTURB}",
                inactive(analysis)
            ));
        }
        LoopBasis::Structure => {
            notes.push(format!(
                "No loop was active in this run: {}, and the model has too many loops to list \
                 from its structure alone. {DISTURB}",
                inactive(analysis)
            ));
        }
        LoopBasis::Run => {
            if !analysis.complete {
                notes.push(
                    "The model has too many loops to enumerate, so these were found by searching \
                     the run for the strongest: a sample, not every loop."
                        .to_string(),
                );
            }
            if let Some(retained) = analysis.capped_from {
                notes.push(format!(
                    "The analysis keeps the {} most important of the run's {retained} loops.",
                    analysis.loops.len()
                ));
            }
            if !spread.is_empty() {
                notes.push(format!(
                    "In the partition{} with {}, the spans whose strongest loop holds less than \
                     a tenth of the loop activity have it spread across many loops: no one loop \
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
        notes.push(match analysis.basis {
            LoopBasis::Run => format!("No loop active in this run goes through {through}."),
            LoopBasis::Structure => format!("No loop goes through {through}."),
        });
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

#[cfg(test)]
#[path = "loops_tests.rs"]
mod tests;
