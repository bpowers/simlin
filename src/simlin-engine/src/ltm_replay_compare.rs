// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every link score of a model two ways: the engine's generated equation's,
//! and [`crate::ltm_replay`]'s, with a name for each way the two differ.
//!
//! A name is a claim that is checked, not a label: [`Why`] is given to a score
//! only where the number it stands for is the generated one, at every step the
//! run saves. What fits none of them is [`Why::Unexplained`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use simlin_base::common::{Canonical, Ident, canonicalize};
use simlin_base::datamodel;
use simlin_base::results::Results;

use crate::analysis::build_link_expansion_context;
use crate::db::{
    LtmOverlay, LtmSyntheticVar, SimlinDb, SourceModel, SourceProject, compile_project_incremental,
    model_ltm_variables, project_datamodel_dims, set_project_ltm_discovery_mode,
    sync_from_datamodel_incremental,
};
use crate::ltm_agg::AGG_NAME_PREFIX;
use crate::ltm_finding::{link_score_offsets, resolve_dim_element_tuples, subscript_from_elements};
use crate::ltm_replay::{Convention, Instance, Misreproduced, Reducers, Replay};
use crate::vm::{CompiledSimulation, Vm};

/// What separates a module instance from what is in it.
const SEPARATOR: char = '\u{B7}';

/// Two scores are the same number to within this, relative.
const SAME: f64 = 1e-9;

/// Why a generated score is not the replayed one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Why {
    /// The two numerators are the same but for the order a sum was added in: a
    /// few last places of the target, over a change of the target that is
    /// itself that small.
    Rounding,
    /// The generated partial leaves every `PREVIOUS` read at the new step, and
    /// takes its sign from the source's own change where the target reads the
    /// source lagged.
    LiveLags,
    /// The generated partial leaves the clock at the new step.
    LiveClock,
    /// The generated partial gives the source its new value last.
    ChangedLast,
    /// The target reads the source in several places, and the generated
    /// partial moves it in one of them.
    OneRead,
    /// The generated score is the target's whole change: 1 or -1.
    WholeChange,
    /// The generated score is 0 throughout.
    Zero,
    /// The generated partial moves every element of the source's variable, and
    /// credits each with what they do together.
    WholeSource,
    /// The generated score of a link into a module instance is the product of
    /// another way through it than the strongest of the ones that lead to what
    /// is read of it, or 1 or -1 where the inside is not looked at.
    OtherPathway,
    /// None of these.
    Unexplained,
}

impl Why {
    /// Every reason, for a test whose rows are the reasons.
    pub const ALL: [Why; 10] = [
        Why::Rounding,
        Why::LiveLags,
        Why::LiveClock,
        Why::ChangedLast,
        Why::OneRead,
        Why::WholeChange,
        Why::Zero,
        Why::WholeSource,
        Why::OtherPathway,
        Why::Unexplained,
    ];
}

/// A link whose generated score is not the replayed one.
#[derive(Clone, Debug)]
pub struct Difference {
    pub from: String,
    pub to: String,
    pub why: Why,
    /// The saved row the two are furthest apart at, and what they are there.
    pub row: usize,
    pub generated: f64,
    pub replayed: f64,
}

/// A link only one of the two scores.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct OneSided {
    pub from: String,
    pub to: String,
}

/// What [`compare`] found.
#[derive(Default, Debug)]
pub struct Comparison {
    /// The generated scores that are the replayed ones at every saved row their
    /// ends are numbers at, a 0 for a link the target does not read included.
    pub agreed: usize,
    /// How many of those are not 0 throughout.
    pub agreed_active: usize,
    pub differences: Vec<Difference>,
    /// The links with a generated score that is not 0 throughout, between two
    /// slots the target of which does not read the source.
    pub not_read: Vec<OneSided>,
    /// The links with a generated score that is not 0 throughout and an end
    /// that is no one slot: a whole array, or a node the model's code has none
    /// of.
    pub not_slots: Vec<OneSided>,
    /// The links between the model's own variables that the target's code
    /// reads, with a replayed score that is not 0 throughout and no generated
    /// score.
    pub not_scored: Vec<OneSided>,
    /// The slots whose code did not give back what the run computed: nothing
    /// is known of a link into one of them.
    pub misreproduced: Vec<Misreproduced>,
}

impl Comparison {
    /// How many links differ for each reason.
    pub fn counts(&self) -> BTreeMap<Why, usize> {
        let mut counts = BTreeMap::new();
        for difference in &self.differences {
            *counts.entry(difference.why).or_insert(0) += 1;
        }
        counts
    }
}

/// Compare every link score of `project`'s main model, its module instances'
/// inner ones included, at every row a run saves. `Err` says why there is
/// nothing to compare.
pub fn compare(project: &datamodel::Project) -> Result<Comparison, String> {
    let main = project
        .models
        .iter()
        .find(|m| m.name == "main" || m.name.is_empty())
        .or_else(|| project.models.iter().find(|m| m.macro_spec.is_none()))
        .ok_or("no model")?;
    let main = datamodel::canonical_model_name(&main.name).into_owned();
    let plain = {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        compile_project_incremental(&db, sync.project, &main, LtmOverlay::Off)
            .map_err(|e| e.to_string())?
    };
    let nodes = Side::new(&plain, Reducers::AsNodes)?;
    let inline = Side::new(&plain, Reducers::Inline)?;
    let generated = Generated::new(project, &main, nodes.replay.instances())?;
    generated.ran_the_same_model(&plain, &nodes.replay)?;
    Ok(Comparer {
        nodes,
        inline,
        generated,
        found: Comparison::default(),
        placed: HashSet::new(),
    }
    .run())
}

/// One link with a generated score.
struct Column {
    from: String,
    to: String,
    offset: usize,
}

/// The run with the generated scores in it.
struct Generated {
    results: Results,
    columns: Vec<Column>,
    /// Each aggregate node, by element for one that has elements, and the
    /// column of its value.
    aggregates: HashMap<String, usize>,
}

impl Generated {
    fn new(
        project: &datamodel::Project,
        main: &str,
        instances: &[Instance],
    ) -> Result<Generated, String> {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        // Every causal link is scored, not only the ones on a loop.
        set_project_ltm_discovery_mode(&mut db, sync.project, true);
        let compiled = compile_project_incremental(&db, sync.project, main, LtmOverlay::On)
            .map_err(|e| e.to_string())?;
        let mut vm = Vm::new(compiled).map_err(|e| e.to_string())?;
        vm.run_to_end().map_err(|e| e.to_string())?;
        let results = vm.into_results();

        let mut columns = Vec::new();
        let mut aggregates = HashMap::new();
        for instance in instances {
            let Some(&model) = sync.project.models(&db).get(instance.model.as_str()) else {
                return Err(format!("no model {}", instance.model));
            };
            let vars = &model_ltm_variables(&db, model, sync.project).vars;
            let (prefix, within) = (&instance.prefix, names_within(&results, instance));
            columns.extend(
                element_links(&db, sync.project, model, vars, &within)
                    .into_iter()
                    .map(|((from, to), offset)| Column {
                        from: format!("{prefix}{}", from.as_str()),
                        to: format!("{prefix}{}", to.as_str()),
                        offset,
                    }),
            );
            aggregates.extend(
                aggregate_columns(&db, sync.project, vars, &within)
                    .into_iter()
                    .map(|(name, offset)| (format!("{prefix}{name}"), offset)),
            );
        }
        columns.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        Ok(Generated {
            results,
            columns,
            aggregates,
        })
    }

    fn series(&self, offset: usize) -> Vec<f64> {
        self.results.iter().map(|row| row[offset]).collect()
    }

    /// The overlay only adds to a model: a run with it and a run without are
    /// the same run, or no score of one says anything of the other.
    fn ran_the_same_model(
        &self,
        plain: &CompiledSimulation,
        replay: &Replay,
    ) -> Result<(), String> {
        if self.results.step_count != replay.saved_steps().len() {
            return Err("the two runs save different rows".to_string());
        }
        for (name, &slot) in &plain.offsets {
            let Some(&column) = self.results.offsets.get(name) else {
                return Err(format!("the run with scores has no {}", name.as_str()));
            };
            let same = self
                .results
                .iter()
                .zip(replay.saved_steps())
                .all(|(row, &step)| same_bits(row[column], replay.value(slot, step)));
            if !same {
                return Err(format!("{} differs between the two runs", name.as_str()));
            }
        }
        Ok(())
    }
}

/// `results`' columns of one instance, by the names its own model knows them
/// by. The rows are left out: the names are all that is read of it.
fn names_within(results: &Results, instance: &Instance) -> Results {
    Results {
        offsets: results
            .offsets
            .iter()
            .filter_map(|(name, &offset)| {
                let name = name.as_str().strip_prefix(instance.prefix.as_str())?;
                Some((Ident::<Canonical>::from_str_unchecked(name), offset))
            })
            .collect(),
        data: Box::default(),
        step_size: results.step_size,
        step_count: results.step_count,
        specs: results.specs.clone(),
        is_vensim: results.is_vensim,
    }
}

/// The links discovery would search, by the engine's own expansion of its
/// score variables into them.
fn element_links(
    db: &SimlinDb,
    project: SourceProject,
    model: SourceModel,
    vars: &[LtmSyntheticVar],
    names: &Results,
) -> Vec<crate::ltm_finding::LinkScoreOffset> {
    link_score_offsets(
        names,
        vars,
        project_datamodel_dims(db, project),
        &build_link_expansion_context(db, model, project),
    )
}

/// Each aggregate node and the column of its value.
fn aggregate_columns(
    db: &SimlinDb,
    project: SourceProject,
    vars: &[LtmSyntheticVar],
    names: &Results,
) -> Vec<(String, usize)> {
    let dims = project_datamodel_dims(db, project);
    let mut out = Vec::new();
    for var in vars {
        let name = canonicalize(&var.name);
        if !name.starts_with(AGG_NAME_PREFIX) {
            continue;
        }
        let Some(&base) = names.offsets.get(&Ident::<Canonical>::new(&name)) else {
            continue;
        };
        match resolve_dim_element_tuples(&var.dimensions, dims) {
            Some(tuples) if !var.dimensions.is_empty() => {
                out.extend(tuples.iter().enumerate().map(|(i, elements)| {
                    (
                        format!("{name}[{}]", subscript_from_elements(elements)),
                        base + i,
                    )
                }));
            }
            _ => out.push((name.into_owned(), base)),
        }
    }
    out
}

/// A replay, its scores at the saved rows, and its edges by their ends.
struct Side {
    replay: Replay,
    scores: Vec<f64>,
    rows: usize,
    between: HashMap<(usize, usize), Vec<usize>>,
    leaving: HashMap<usize, Vec<usize>>,
    slots: HashMap<String, usize>,
}

impl Side {
    fn new(plain: &Arc<CompiledSimulation>, reducers: Reducers) -> Result<Side, String> {
        let mut replay = Replay::new(plain, reducers)?;
        let steps = replay.saved_steps().to_vec();
        let scores = replay.scores(&steps);
        let mut between: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        let mut leaving: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, edge) in replay.edges().iter().enumerate() {
            between.entry((edge.from, edge.to)).or_default().push(i);
            leaving.entry(edge.from).or_default().push(i);
        }
        let slots = plain
            .offsets
            .iter()
            .map(|(name, &slot)| (name.as_str().to_string(), slot))
            .collect();
        Ok(Side {
            replay,
            scores,
            rows: steps.len(),
            between,
            leaving,
            slots,
        })
    }

    fn series(&self, edge: usize) -> &[f64] {
        &self.scores[edge * self.rows..(edge + 1) * self.rows]
    }

    /// The edges from `from` to `to`, where a source that is an instance's
    /// input port is also the slot the port copies: the code inside reads the
    /// one, the generated score names the other.
    fn edges_between(&self, from: usize, to: usize) -> Vec<usize> {
        let source = self.replay.port_source(from);
        [Some(from), source]
            .into_iter()
            .flatten()
            .filter_map(|from| self.between.get(&(from, to)))
            .flatten()
            .copied()
            .collect()
    }

    fn name(&self, slot: usize) -> &str {
        self.replay.name(slot).unwrap_or_default()
    }

    /// Whether a slot is inside the module instance `instance`.
    fn is_inside(&self, slot: usize, instance: &str) -> bool {
        self.name(slot)
            .strip_prefix(instance)
            .is_some_and(|rest| rest.starts_with(SEPARATOR))
    }

    /// The slots of an instance that something outside it reads, or its
    /// `output` where nothing does.
    fn exits(&self, instance: &str) -> HashSet<usize> {
        let is_read_outside = |slot: &usize| {
            self.leaving.get(slot).is_some_and(|edges| {
                edges
                    .iter()
                    .any(|&e| !self.is_inside(self.replay.edges()[e].to, instance))
            })
        };
        let mut exits: HashSet<usize> = self.inside(instance).filter(is_read_outside).collect();
        if exits.is_empty() {
            exits.extend(self.slots.get(&format!("{instance}{SEPARATOR}output")));
        }
        exits
    }

    fn inside(&self, instance: &str) -> impl Iterator<Item = usize> {
        (0..self.replay.width()).filter(move |&slot| self.is_inside(slot, instance))
    }

    /// Every simple path from one of `starts` through the inside of `instance`
    /// to one of `ends`, as edges.
    fn pathways(
        &self,
        starts: &BTreeSet<usize>,
        instance: &str,
        ends: &HashSet<usize>,
    ) -> Vec<Vec<usize>> {
        // No instance of a model in the corpus has more; a search this bounds
        // would otherwise answer with some of the paths as though all.
        const MOST: usize = 10_000;
        let mut found: Vec<Vec<usize>> = Vec::new();
        for &start in starts {
            // The edges taken so far, and at each of the slots they pass which
            // of its edges is tried next.
            let mut path: Vec<usize> = Vec::new();
            let mut passed = vec![start];
            let mut next = vec![0usize];
            while let Some(&at) = passed.last() {
                let turn = next.last_mut().expect("one turn per slot passed");
                let edge = self.leaving.get(&at).and_then(|edges| edges.get(*turn));
                *turn += 1;
                let Some(&edge) = edge else {
                    passed.pop();
                    next.pop();
                    path.pop();
                    continue;
                };
                let to = self.replay.edges()[edge].to;
                if !self.is_inside(to, instance) || passed.contains(&to) {
                    continue;
                }
                path.push(edge);
                passed.push(to);
                next.push(0);
                if ends.contains(&to) {
                    found.push(path.clone());
                    assert!(found.len() <= MOST, "too many pathways through {instance}");
                }
            }
        }
        found
    }

    /// The product of the scores along a path, at each saved row.
    fn product(&self, path: &[usize]) -> Vec<f64> {
        (0..self.rows)
            .map(|row| path.iter().map(|&edge| self.series(edge)[row]).product())
            .collect()
    }

    /// The saved rows at which both ends of a link are numbers, then and a
    /// step earlier: a change to or from what is not a number has no score.
    fn defined(&self, from: usize, to: usize) -> Vec<bool> {
        (self.replay.saved_steps().iter())
            .map(|&step| {
                [step, step.saturating_sub(1), step.saturating_sub(2)]
                    .iter()
                    .all(|&s| {
                        self.replay.value(from, s).is_finite()
                            && self.replay.value(to, s).is_finite()
                    })
            })
            .collect()
    }
}

/// Which replay an edge is of.
#[derive(Clone, Copy, PartialEq)]
enum Which {
    Nodes,
    Inline,
}

struct Comparer {
    nodes: Side,
    inline: Side,
    generated: Generated,
    found: Comparison,
    /// The links that have a generated score, by name.
    placed: HashSet<(String, String)>,
}

/// One edge that a generated score may be of, and how far from it it is:
/// `None` when they are the same at every row there is anything to compare.
struct Candidate {
    which: Which,
    edge: usize,
    off: Option<Apart>,
    /// Whether there is anything to compare.
    is_defined: bool,
}

/// How far apart two series get, and the row that is at.
#[derive(Clone, Copy)]
struct Apart {
    by: f64,
    row: usize,
}

impl Comparer {
    fn run(mut self) -> Comparison {
        self.found.misreproduced = self.nodes.replay.misreproduced();
        (self.found.misreproduced).extend(self.inline.replay.misreproduced());
        let aggregates = self.match_aggregates();
        for column in std::mem::take(&mut self.generated.columns) {
            self.placed.insert((column.from.clone(), column.to.clone()));
            let slot = |name: &str| {
                aggregates
                    .get(name)
                    .or_else(|| self.nodes.slots.get(name))
                    .copied()
            };
            let series = self.generated.series(column.offset);
            let (from, to) = (slot(&column.from), slot(&column.to));
            match (from, to) {
                (Some(from), Some(to)) => self.link(&column, &series, from, to),
                (None, Some(to)) if self.is_instance(&column.from) => {
                    self.link_out_of_instance(&column, &series, to)
                }
                (from, None) if self.is_instance(&column.to) => {
                    self.link_into_instance(&column, &series, from)
                }
                _ if is_active(&series) => self.found.not_slots.push(one_sided(&column)),
                _ => {}
            }
        }
        self.unscored(&aggregates);
        self.found
            .differences
            .sort_by(|a, b| (a.why, &a.from, &a.to).cmp(&(b.why, &b.from, &b.to)));
        self.found.not_read.sort();
        self.found.not_slots.sort();
        self.found.not_scored.sort();
        self.found.not_scored.dedup();
        self.found
    }

    fn is_instance(&self, name: &str) -> bool {
        self.nodes.slots.keys().any(|slot| {
            slot.strip_prefix(name)
                .is_some_and(|rest| rest.starts_with(SEPARATOR))
        })
    }

    /// The replay's slot for each generated aggregate node: the one that has
    /// its value at every row, and of several the one whose reads and readers
    /// are most the node's.
    fn match_aggregates(&self) -> HashMap<String, usize> {
        let side = &self.nodes;
        let mut neighbours: HashMap<&str, HashSet<&str>> = HashMap::new();
        for column in &self.generated.columns {
            for (end, other) in [(&column.from, &column.to), (&column.to, &column.from)] {
                neighbours.entry(end).or_default().insert(other);
            }
        }
        let steps = side.replay.saved_steps();
        let mut matched = HashMap::new();
        for (name, &offset) in &self.generated.aggregates {
            let series = self.generated.series(offset);
            let best = side
                .replay
                .reads()
                .filter(|(target, _)| {
                    series
                        .iter()
                        .zip(steps)
                        .all(|(&v, &step)| apart(v, side.replay.value(*target, step)) <= 1e-12)
                })
                .map(|(target, reads)| {
                    let readers = side.leaving.get(&target).into_iter().flatten();
                    let shared = reads
                        .iter()
                        .map(|read| read.slot)
                        .chain(readers.map(|&edge| side.replay.edges()[edge].to))
                        .filter(|&slot| {
                            neighbours
                                .get(name.as_str())
                                .is_some_and(|names| names.contains(side.name(slot)))
                        })
                        .count();
                    (shared, std::cmp::Reverse(target))
                })
                .max();
            if let Some((_, std::cmp::Reverse(slot))) = best {
                matched.insert(name.clone(), slot);
            }
        }
        matched
    }

    /// A link between two slots.
    fn link(&mut self, column: &Column, series: &[f64], from: usize, to: usize) {
        // An aggregate node that is a variable's whole equation, as a rank is
        // whose elements are no reducer's: the variable is it.
        if from == to && column.from != column.to {
            let values = self.nodes.replay.saved_steps().iter().map(|&step| {
                let before = step.saturating_sub(1);
                self.nodes.replay.value(to, step) - self.nodes.replay.value(to, before)
            });
            let itself: Vec<f64> = values.map(|dz| f64::from(dz != 0.0)).collect();
            let defined = self.nodes.defined(from, to);
            return match furthest(series, &itself, &defined) {
                _ if !defined.contains(&true) => {}
                None => self.agreed(series),
                Some(Apart { row, .. }) => {
                    self.differs(column, series, Why::Unexplained, row, itself[row])
                }
            };
        }
        let is_node = |slot: usize| self.nodes.replay.is_reducer_node(slot);
        let mut candidates = self.candidates(
            Which::Nodes,
            self.nodes.edges_between(from, to),
            series,
            from,
            to,
        );
        if !is_node(from) && !is_node(to) {
            // A reducer the engine made no node of is part of the equation it
            // is in, so the same link of the other replay answers for it. Where
            // it did make one, what the source does is in the links through
            // the node, and the link past it does nothing.
            if candidates.is_empty() && !is_active(series) {
                return self.agreed(series);
            }
            candidates.extend(self.candidates(
                Which::Inline,
                self.inline.edges_between(from, to),
                series,
                from,
                to,
            ));
        }
        if candidates.is_empty() {
            if is_active(series) {
                self.found.not_read.push(one_sided(column));
            }
            return;
        }
        self.settle(column, series, candidates);
    }

    /// A link from a module instance to a variable: the variable's reads of
    /// the instance's slots, of which it is one.
    fn link_out_of_instance(&mut self, column: &Column, series: &[f64], to: usize) {
        let mut candidates = Vec::new();
        for which in [Which::Nodes, Which::Inline] {
            let side = self.side(which);
            for (i, edge) in side.replay.edges().iter().enumerate() {
                if edge.to == to && side.is_inside(edge.from, &column.from) {
                    candidates.extend(self.candidates(which, vec![i], series, edge.from, to));
                }
            }
        }
        if candidates.is_empty() {
            if is_active(series) {
                self.found.not_read.push(one_sided(column));
            }
            return;
        }
        self.settle(column, series, candidates);
    }

    fn candidates(
        &self,
        which: Which,
        edges: Vec<usize>,
        series: &[f64],
        from: usize,
        to: usize,
    ) -> Vec<Candidate> {
        let side = self.side(which);
        let defined = side.defined(from, to);
        edges
            .into_iter()
            .map(|edge| Candidate {
                which,
                edge,
                off: furthest(series, side.series(edge), &defined),
                is_defined: defined.contains(&true),
            })
            .collect()
    }

    /// Whether a generated score is one of `candidates`', and if it is none,
    /// what it is of the nearest.
    fn settle(&mut self, column: &Column, series: &[f64], mut candidates: Vec<Candidate>) {
        // Ends that are never numbers leave nothing to say of a link.
        candidates.retain(|c| c.is_defined);
        candidates.sort_by(|a, b| {
            let by = |c: &Candidate| c.off.map_or(0.0, |off| off.by);
            by(a).total_cmp(&by(b))
        });
        let Some(nearest) = candidates.first() else {
            return;
        };
        let Some(Apart { row, .. }) = nearest.off else {
            return self.agreed(series);
        };
        let replayed = self.side(nearest.which).series(nearest.edge)[row];
        let why = candidates
            .iter()
            .map(|c| self.explain(c, series))
            .find(|&why| why != Why::Unexplained)
            .unwrap_or(Why::Unexplained);
        self.differs(column, series, why, row, replayed);
    }

    /// A link into a module instance: the product of the scores along the
    /// strongest of the ways through it.
    fn link_into_instance(&mut self, column: &Column, series: &[f64], from: Option<usize>) {
        let side = &self.nodes;
        let starts: BTreeSet<usize> = match from {
            Some(from) => [Some(from), side.replay.port_source(from)]
                .into_iter()
                .flatten()
                .collect(),
            // From an instance to an instance: the slots of the first that the
            // second reads.
            None => side
                .replay
                .edges()
                .iter()
                .filter(|e| {
                    side.is_inside(e.from, &column.from) && side.is_inside(e.to, &column.to)
                })
                .map(|e| e.from)
                .collect(),
        };
        let exits = side.exits(&column.to);
        let paths = side.pathways(&starts, &column.to, &exits);
        if paths.is_empty() {
            if is_active(series) {
                self.found.not_read.push(one_sided(column));
            }
            return;
        }
        let mut defined = vec![true; series.len()];
        for &edge in paths.iter().flatten() {
            let edge = side.replay.edges()[edge];
            for (all, one) in defined.iter_mut().zip(side.defined(edge.from, edge.to)) {
                *all &= one;
            }
        }
        let products: Vec<Vec<f64>> = paths.iter().map(|path| side.product(path)).collect();
        let strongest: Vec<f64> = (0..series.len())
            .map(|row| {
                products
                    .iter()
                    .map(|product| product[row])
                    .fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a })
            })
            .collect();
        if !defined.contains(&true) {
            return;
        }
        let Some(Apart { row, .. }) = furthest(series, &strongest, &defined) else {
            return self.agreed(series);
        };
        let is_a_path = || {
            side.pathways(&starts, &column.to, &side.inside(&column.to).collect())
                .iter()
                .any(|path| furthest(series, &side.product(path), &defined).is_none())
        };
        let why = if !is_active(series) {
            Why::Zero
        } else if series.iter().all(|v| *v == 0.0 || v.abs() == 1.0) || is_a_path() {
            Why::OtherPathway
        } else {
            Why::Unexplained
        };
        self.differs(column, series, why, row, strongest[row]);
    }

    /// The number a generated score stands for, where it is not the replayed
    /// one.
    fn explain(&mut self, candidate: &Candidate, series: &[f64]) -> Why {
        let edge = candidate.edge;
        let side = match candidate.which {
            Which::Nodes => &mut self.nodes,
            Which::Inline => &mut self.inline,
        };
        let (from, to) = {
            let e = side.replay.edges()[edge];
            (e.from, e.to)
        };
        let rows: Vec<(usize, usize)> = side
            .defined(from, to)
            .iter()
            .zip(side.replay.saved_steps())
            .enumerate()
            .filter(|(_, (defined, _))| **defined)
            .map(|(row, (_, &step))| (row, step))
            .collect();
        let is_rounding = rows.iter().all(|&(row, step)| {
            let (now, before) = (
                side.replay.value(to, step),
                side.replay.value(to, step.saturating_sub(1)),
            );
            let off = (series[row] - side.series(edge)[row]).abs() * (now - before).abs();
            off <= 64.0 * f64::EPSILON * now.abs().max(before.abs())
                || apart(series[row], side.series(edge)[row]) <= SAME
        });
        if is_rounding {
            return Why::Rounding;
        }
        let is_near = |score: f64, v: f64| apart(score, v) <= 1e-7 || (score - v).abs() < 1e-12;
        let mut answers = |convention: Convention| {
            rows.iter().all(|&(row, step)| {
                let generated = if convention.is_unsigned() {
                    series[row].abs()
                } else {
                    series[row]
                };
                side.replay
                    .score_under(edge, step, convention)
                    .is_some_and(|score| is_near(score, generated))
            })
        };
        // The order is from the most particular claim to the least: a score
        // that is 0 throughout answers the last ones by accident.
        if answers(Convention::LiveLags) || answers(Convention::LiveLagsCurrentSign) {
            return Why::LiveLags;
        }
        if answers(Convention::LiveClock) {
            return Why::LiveClock;
        }
        if answers(Convention::ChangedLast) {
            return Why::ChangedLast;
        }
        let is_whole_change = answers(Convention::WholeChange);
        let is_whole_source = is_active(series)
            && (answers(Convention::WholeSource) || answers(Convention::WholeSourceChangedLast));
        let loads = side.replay.loads(edge);
        let is_one_read = loads > 1
            && (0..loads).any(|load| {
                rows.iter().all(|&(row, step)| {
                    side.replay
                        .score_of_one_load(edge, step, load)
                        .is_some_and(|score| is_near(score, series[row]))
                })
            });
        if is_one_read {
            Why::OneRead
        } else if is_whole_change {
            Why::WholeChange
        } else if !is_active(series) {
            Why::Zero
        } else if is_whole_source {
            Why::WholeSource
        } else {
            Why::Unexplained
        }
    }

    fn side(&self, which: Which) -> &Side {
        match which {
            Which::Nodes => &self.nodes,
            Which::Inline => &self.inline,
        }
    }

    fn agreed(&mut self, series: &[f64]) {
        self.found.agreed += 1;
        if is_active(series) {
            self.found.agreed_active += 1;
        }
    }

    fn differs(&mut self, column: &Column, series: &[f64], why: Why, row: usize, replayed: f64) {
        self.found.differences.push(Difference {
            from: column.from.clone(),
            to: column.to.clone(),
            why,
            row,
            generated: series[row],
            replayed,
        });
    }

    /// The reads between the model's own variables that no generated score is
    /// of: neither the link itself nor, where it passes through reducer nodes,
    /// each of the links it is made of. The reads inside a module instance,
    /// through a port or of a helper are parts of links between variables, not
    /// links.
    fn unscored(&mut self, aggregates: &HashMap<String, usize>) {
        // A model with no state has no loops, and the engine scores none of it.
        if self.placed.is_empty() {
            return;
        }
        let side = &self.nodes;
        let mut names: HashMap<usize, Vec<&str>> = HashMap::new();
        for (name, &slot) in aggregates {
            names.entry(slot).or_default().push(name);
        }
        let is_scored = |from: usize, to: usize| {
            let named = |slot: usize| {
                let own = side.name(slot);
                let also = names.get(&slot).into_iter().flatten().copied();
                also.chain([own]).map(str::to_string).collect::<Vec<_>>()
            };
            named(from).iter().any(|from| {
                named(to)
                    .iter()
                    .any(|to| self.placed.contains(&(from.clone(), to.clone())))
            })
        };
        let is_variable = |slot: usize| {
            side.replay
                .name(slot)
                .is_some_and(|name| !name.contains(SEPARATOR) && !name.starts_with(['$', '<']))
        };
        let edges = side.replay.edges();
        for (i, first) in edges.iter().enumerate() {
            if !is_variable(first.from) || !is_active(side.series(i)) {
                continue;
            }
            // Through reducer nodes to each variable they are read by.
            let mut work = vec![(first.to, is_scored(first.from, first.to))];
            while let Some((at, so_far)) = work.pop() {
                if is_variable(at) {
                    if !so_far && !is_scored(first.from, at) {
                        self.found.not_scored.push(OneSided {
                            from: side.name(first.from).to_string(),
                            to: side.name(at).to_string(),
                        });
                    }
                } else if side.replay.is_reducer_node(at) {
                    for &e in side.leaving.get(&at).into_iter().flatten() {
                        if is_active(side.series(e)) {
                            work.push((edges[e].to, so_far && is_scored(at, edges[e].to)));
                        }
                    }
                }
            }
        }
    }
}

fn one_sided(column: &Column) -> OneSided {
    OneSided {
        from: column.from.clone(),
        to: column.to.clone(),
    }
}

/// Whether a score is not 0 throughout.
fn is_active(series: &[f64]) -> bool {
    series.iter().any(|v| *v != 0.0)
}

/// How far apart two numbers are, relative to the larger.
fn apart(a: f64, b: f64) -> f64 {
    if a == b || (a.is_nan() && b.is_nan()) {
        0.0
    } else if !a.is_finite() || !b.is_finite() {
        f64::INFINITY
    } else {
        (a - b).abs() / a.abs().max(b.abs())
    }
}

/// Where two series are furthest apart of the rows they are defined at, `None`
/// where they are the same at every one.
fn furthest(a: &[f64], b: &[f64], defined: &[bool]) -> Option<Apart> {
    let mut worst: Option<Apart> = None;
    for (row, ((&a, &b), &defined)) in a.iter().zip(b).zip(defined).enumerate() {
        let by = apart(a, b);
        if defined && by > SAME && worst.is_none_or(|worst| by > worst.by) {
            worst = Some(Apart { by, row });
        }
    }
    worst
}

fn same_bits(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[cfg(test)]
#[path = "ltm_replay_compare_tests.rs"]
mod tests;
