// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Link scores by replaying a variable's own compiled code: an account of
//! LTM's numbers that shares no equation with the one the engine generates.
//!
//! A link score asks how much of a target's change over one step its source's
//! change accounts for, every other input held:
//!
//! ```text
//! (z(x now, the rest before) - z before) / |z now - z before| * sign(x now - x before)
//! ```
//!
//! and is 0 at a step where either end does not change. The engine answers by
//! generating an equation for the first term (the target's equation with every
//! other read wrapped in `PREVIOUS`) and compiling that beside the model.
//! [`Replay`] runs the model as it is, and evaluates the first term by running
//! the code the model already has for `z`, once, over the row of the step
//! before with the one slot `x` taken from this step's row. Nothing is
//! generated, parsed or lowered, so the two can only agree by both being right
//! about what the target reads.
//!
//! The model's flows program is cut at its assignments into one [`Unit`] per
//! slot it writes, flattened over module instances. Two things are read off
//! the code rather than trusted:
//!
//! - *What a unit reads* is every slot its opcodes can address. An opcode this
//!   module has no rule for is an error, never a read that is missed, and
//!   [`Replay::misreproduced`] re-runs every unit over rows that hold nothing
//!   of the run but its reads, which must give back what the run computed.
//! - *A stock's update* is a sum of flow slots, read symbolically. Its score
//!   from a flow is the share of the stock's net flow: the same definition,
//!   with the net flow as the target.
//!
//! Scores are of slots. A variable-level reading of the same link (every
//! element of the source at once, a lag and its source as one) is another
//! question, and [`Convention`] holds the ones the generated scores answer
//! instead, so a comparison can say which.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use smallvec::SmallVec;

use simlin_base::results::{DT_OFF, IMPLICIT_VAR_COUNT, TIME_OFF};

use crate::bytecode::{
    ByteCode, ByteCodeContext, CompiledModule, DimId, Op2, Opcode, RuntimeView, StaticArrayView,
    ViewStorage,
};
use crate::vm::{
    CompiledSimulation, ModuleKey, PieceEvaluator, Vm, increment_indices, make_module_key,
};

/// Which row a read addresses: the step's own or the one before it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Lag {
    Current,
    Previous,
}

/// One slot a unit's code can read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Read {
    pub slot: usize,
    pub lag: Lag,
}

/// The code that computes one slot, and what it reads.
struct Unit {
    target: usize,
    reads: Vec<Read>,
    code: ByteCode,
    context: Arc<ByteCodeContext>,
    module_off: usize,
}

/// A stock's update: `next = stock + dt * (sum(inflows) - sum(outflows))`.
struct StockUpdate {
    target: usize,
    inflows: Vec<usize>,
    outflows: Vec<usize>,
}

impl StockUpdate {
    fn net(&self, row: &[f64]) -> f64 {
        // The order the stock's own code adds in, so the sum rounds as it does.
        let sum = |flows: &[usize]| -> f64 {
            let mut values = flows.iter().map(|&f| row[f]);
            values
                .next()
                .map_or(0.0, |first| values.fold(first, |a, b| a + b))
        };
        sum(&self.inflows) - sum(&self.outflows)
    }
}

/// What computes an edge's target.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Head {
    Unit(usize),
    /// A flow into (`1.0`) or out of (`-1.0`) the stock of this update.
    Stock(usize, f64),
}

/// One slot read by the code of another.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub lag: Lag,
    head: Head,
}

impl Edge {
    /// Whether this is a flow's edge to its stock.
    pub fn is_flow_to_stock(&self) -> bool {
        matches!(self.head, Head::Stock(..))
    }
}

/// How a reducer (`SUM`, `MEAN`, ...) inside a larger equation is treated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reducers {
    /// Part of the equation it is in: its elements are the target's inputs.
    Inline,
    /// A node of its own between its elements and the target, as LTM's
    /// aggregate nodes are.
    AsNodes,
}

/// A way of scoring a link. [`Convention::ChangedFirst`] is the definition;
/// the rest are what a generated score is known to answer instead, kept to
/// name why a score differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Convention {
    /// The source takes its new value first: `z(x now, rest before) - z before`.
    ChangedFirst,
    /// The source takes its new value last: `z now - z(x before, rest now)`.
    ChangedLast,
    /// Changed-first, with the clock left at the new step.
    LiveClock,
    /// Changed-first, with every `PREVIOUS` read left at the new step.
    LiveLags,
    /// [`Convention::LiveLags`], the sign taken from the source's own change
    /// where the target reads it lagged.
    LiveLagsCurrentSign,
    /// The whole change of the target is credited to the source.
    WholeChange,
    /// Changed-first, with every element of the source's variable that the
    /// target reads taking its new value at once.
    WholeSource,
    /// Changed-last, with every element of the source's variable that the
    /// target reads held back at once.
    WholeSourceChangedLast,
}

impl Convention {
    /// Every convention, for a test whose rows are the conventions.
    pub const ALL: [Convention; 8] = [
        Convention::ChangedFirst,
        Convention::ChangedLast,
        Convention::LiveClock,
        Convention::LiveLags,
        Convention::LiveLagsCurrentSign,
        Convention::WholeChange,
        Convention::WholeSource,
        Convention::WholeSourceChangedLast,
    ];

    /// Whether the score's sign is not part of what this convention states.
    pub fn is_unsigned(self) -> bool {
        matches!(
            self,
            Convention::WholeSource | Convention::WholeSourceChangedLast
        )
    }
}

/// One module instance the model is made of.
#[derive(Clone, Debug)]
pub struct Instance {
    /// The instance's name as a prefix of its variables' (`inst·`), empty for
    /// the model itself.
    pub prefix: String,
    /// The model it instantiates, by canonical name.
    pub model: String,
}

/// A unit whose code, run over its own reads, does not give back the value the
/// model's run computed.
#[derive(Clone, Debug)]
pub struct Misreproduced {
    pub name: String,
    pub step: usize,
    pub ran: f64,
    pub replayed: f64,
}

/// The rows a piece of code is evaluated over.
struct Frame {
    curr: Vec<f64>,
    prev: Vec<f64>,
    at_first_step: bool,
}

/// A model's run, and the code of each of its slots to replay over it.
pub struct Replay {
    n_slots: usize,
    /// The model's slots and then one per reducer node.
    width: usize,
    units: Vec<Unit>,
    stocks: Vec<StockUpdate>,
    /// The reducer nodes' units, each after the ones it reads.
    reducers: Vec<usize>,
    edges: Vec<Edge>,
    names: Vec<Option<String>>,
    /// A bound input port's slot, and the slot it copies.
    ports: HashMap<usize, usize>,
    instances: Vec<Instance>,
    /// Every step's row, `width` wide.
    rows: Vec<f64>,
    saved_steps: Vec<usize>,
    evaluator: PieceEvaluator,
}

impl Replay {
    /// Cut `sim`'s program and run it, keeping every step.
    ///
    /// `Err` says what in the program has no rule here.
    pub fn new(sim: &Arc<CompiledSimulation>, reducers: Reducers) -> Result<Replay, String> {
        let n_slots = sim.n_slots();
        let mut cutter = Cutter {
            sim,
            reducers,
            width: n_slots,
            units: Vec::new(),
            stocks: Vec::new(),
            reducer_units: Vec::new(),
            reducer_slots: HashMap::new(),
            reducer_names: Vec::new(),
            ports: HashMap::new(),
            instances: Vec::new(),
        };
        cutter.instance(&sim.root, 0, &[])?;
        let Cutter {
            width,
            units,
            stocks,
            reducer_units,
            reducer_names,
            ports,
            instances,
            ..
        } = cutter;

        let mut written: HashSet<usize> = HashSet::new();
        for target in units
            .iter()
            .map(|u| u.target)
            .chain(stocks.iter().map(|s| s.target))
        {
            if !written.insert(target) {
                return Err(format!("slot {target} is computed in two places"));
            }
        }

        let mut names: Vec<Option<String>> = vec![None; n_slots];
        for (name, &slot) in &sim.offsets {
            names[slot] = Some(name.as_str().to_string());
        }
        names.extend(reducer_names.into_iter().map(Some));
        let instances = instances
            .into_iter()
            .map(|(base, n, model)| Instance {
                prefix: instance_prefix(&names[..n_slots], base, n),
                model,
            })
            .collect();

        let mut specs = sim.specs.clone();
        if let Some(reason) = specs.refusal() {
            return Err(reason.to_string());
        }
        let n_steps = specs.final_step() as usize + 1;
        specs.save_step = specs.dt;
        specs.n_chunks = n_steps;
        let mut vm = Vm::with_specs(sim.clone(), specs).map_err(|e| e.to_string())?;
        vm.run_to_end().map_err(|e| e.to_string())?;
        let evaluator = PieceEvaluator::new(&vm);
        let results = vm.into_results();
        assert_eq!(results.step_count, n_steps, "the run saves every step");
        let mut rows = vec![0.0; n_steps * width];
        for (row, saved) in rows
            .chunks_exact_mut(width)
            .zip(results.data.chunks_exact(n_slots))
        {
            row[..n_slots].copy_from_slice(saved);
        }

        let edges = edges_of(&units, &stocks);
        let saved_steps = (0..sim.specs.n_chunks as u64)
            .map(|row| sim.specs.saved_row_step(row) as usize)
            .collect();
        let mut replay = Replay {
            n_slots,
            width,
            units,
            stocks,
            reducers: reducer_units,
            edges,
            names,
            ports,
            instances,
            rows,
            saved_steps,
            evaluator,
        };
        replay.fill_reducer_nodes();
        Ok(replay)
    }

    /// Every edge of the model.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// How many steps the run took, the first included.
    pub fn n_steps(&self) -> usize {
        self.rows.len() / self.width
    }

    /// The step each row the model saves is of.
    pub fn saved_steps(&self) -> &[usize] {
        &self.saved_steps
    }

    /// Every module instance, the model itself first.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// A slot's value at a step.
    pub fn value(&self, slot: usize, step: usize) -> f64 {
        self.row(step)[slot]
    }

    /// The results name of a slot, `None` for one that has none (a capture
    /// helper's, a lookup-only table's). A reducer node's is `<sum 0>`.
    pub fn name(&self, slot: usize) -> Option<&str> {
        self.names[slot].as_deref()
    }

    /// How many slots there are: the model's, then the reducer nodes'.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Whether a slot is a reducer node's.
    pub fn is_reducer_node(&self, slot: usize) -> bool {
        slot >= self.n_slots
    }

    /// The slot a bound input port copies, if `slot` is one.
    pub fn port_source(&self, slot: usize) -> Option<usize> {
        self.ports.get(&slot).copied()
    }

    /// Each slot that has code, and what the code reads. A stock has none.
    pub fn reads(&self) -> impl Iterator<Item = (usize, &[Read])> {
        self.units.iter().map(|u| (u.target, u.reads.as_slice()))
    }

    /// The slots whose own code, over a row that holds only what it reads,
    /// does not give back the run's value at some step, each at the first.
    ///
    /// Empty says two things: the code was cut where one slot's ends and the
    /// next one's begins, and the reads are all of them.
    pub fn misreproduced(&mut self) -> Vec<Misreproduced> {
        // What the row holds where the unit reads nothing. A sum shows a read
        // of the first; a sort, which may leave a NaN where it stands, shows
        // one of the others.
        const ELSEWHERE: [f64; 3] = [f64::NAN, 1e300, -1e300];
        let mut found: BTreeMap<usize, Misreproduced> = BTreeMap::new();
        for elsewhere in ELSEWHERE {
            let mut frame = Frame {
                curr: vec![elsewhere; self.width],
                prev: vec![elsewhere; self.width],
                at_first_step: true,
            };
            for step in 0..self.n_steps() {
                frame.at_first_step = step == 0;
                let now = step * self.width;
                frame.curr[..IMPLICIT_VAR_COUNT]
                    .copy_from_slice(&self.rows[now..now + IMPLICIT_VAR_COUNT]);
                for unit in &self.units {
                    for read in &unit.reads {
                        match read.lag {
                            Lag::Current => frame.curr[read.slot] = self.rows[now + read.slot],
                            Lag::Previous if step > 0 => {
                                frame.prev[read.slot] = self.rows[now - self.width + read.slot];
                            }
                            Lag::Previous => {}
                        }
                    }
                    let replayed = run(&mut self.evaluator, unit, &mut frame);
                    for read in &unit.reads {
                        match read.lag {
                            Lag::Current => frame.curr[read.slot] = elsewhere,
                            Lag::Previous => frame.prev[read.slot] = elsewhere,
                        }
                    }
                    frame.curr[unit.target] = elsewhere;
                    let ran = self.rows[now + unit.target];
                    if replayed.to_bits() != ran.to_bits() && !(replayed.is_nan() && ran.is_nan()) {
                        found.entry(unit.target).or_insert_with(|| Misreproduced {
                            name: self.names[unit.target].clone().unwrap_or_default(),
                            step,
                            ran,
                            replayed,
                        });
                    }
                }
            }
        }
        found.into_values().collect()
    }

    /// Every edge's score at each of `steps`: `[edge * steps.len() + step]`.
    pub fn scores(&mut self, steps: &[usize]) -> Vec<f64> {
        let mut out = vec![0.0; self.edges.len() * steps.len()];
        for (si, &step) in steps.iter().enumerate() {
            if step == 0 {
                continue;
            }
            let mut frame = self.frame(step - 1);
            for (ei, edge) in self.edges.iter().enumerate() {
                out[ei * steps.len() + si] = changed_first(
                    edge,
                    Rows {
                        data: &self.rows,
                        width: self.width,
                    },
                    &self.units,
                    &self.stocks,
                    &mut self.evaluator,
                    &mut frame,
                    step,
                );
            }
        }
        out
    }

    /// One edge's score at a step under `convention`. A flow's edge to its
    /// stock has one convention, and so answers none of the others.
    pub fn score_under(&mut self, edge: usize, step: usize, convention: Convention) -> Option<f64> {
        let edge = self.edges[edge];
        let Head::Unit(unit) = edge.head else {
            return None;
        };
        if step == 0 {
            return Some(0.0);
        }
        let (x, z) = (edge.from, edge.to);
        let older = step.saturating_sub(2);
        let dz = self.value(z, step) - self.value(z, step - 1);
        let dx = if edge.lag == Lag::Current || convention == Convention::LiveLagsCurrentSign {
            self.value(x, step) - self.value(x, step - 1)
        } else {
            self.value(x, step - 1) - self.value(x, older)
        };
        if dz == 0.0 || dx == 0.0 {
            return Some(0.0);
        }
        let whole: Vec<usize> = self.units[unit]
            .reads
            .iter()
            .map(|r| r.slot)
            .filter(|&slot| self.variable(slot) == self.variable(x))
            .collect();

        let last = matches!(
            convention,
            Convention::ChangedLast | Convention::WholeSourceChangedLast
        );
        let mut frame = self.frame(if last { step } else { step - 1 });
        // Put one slot of the frame at the other end of the step.
        let move_slot = |frame: &mut Frame, slot: usize, lag: Lag| match (lag, last) {
            (Lag::Current, false) => frame.curr[slot] = self.value(slot, step),
            (Lag::Current, true) => frame.curr[slot] = self.value(slot, step - 1),
            (Lag::Previous, false) => {
                frame.prev[slot] = self.value(slot, step - 1);
                frame.at_first_step = false;
            }
            (Lag::Previous, true) => frame.prev[slot] = self.value(slot, older),
        };
        match convention {
            Convention::ChangedFirst | Convention::ChangedLast => {
                move_slot(&mut frame, x, edge.lag);
            }
            Convention::LiveClock => {
                move_slot(&mut frame, x, edge.lag);
                frame.curr[TIME_OFF] = self.value(TIME_OFF, step);
            }
            Convention::LiveLags | Convention::LiveLagsCurrentSign => {
                move_slot(&mut frame, x, edge.lag);
                frame.prev.copy_from_slice(self.row(step - 1));
                frame.at_first_step = false;
            }
            Convention::WholeChange => return Some(dz / dz.abs() * sign(dx)),
            Convention::WholeSource | Convention::WholeSourceChangedLast => {
                for &slot in &whole {
                    move_slot(&mut frame, slot, Lag::Current);
                    move_slot(&mut frame, slot, Lag::Previous);
                }
            }
        }
        let replayed = run(&mut self.evaluator, &self.units[unit], &mut frame);
        let change = if last {
            self.value(z, step) - replayed
        } else {
            replayed - self.value(z, step - 1)
        };
        Some(if convention.is_unsigned() {
            (change / dz).abs()
        } else {
            change / dz.abs() * sign(dx)
        })
    }

    /// How many of the target's reads of an edge's source can be told apart:
    /// its plain loads of the slot. A read through a view or a computed
    /// subscript is of every element at once.
    pub fn loads(&self, edge: usize) -> usize {
        let edge = self.edges[edge];
        match edge.head {
            Head::Unit(unit) => self.units[unit]
                .code
                .code
                .iter()
                .filter(|op| loads(op, &self.units[unit], edge.from, edge.lag))
                .count(),
            Head::Stock(..) => 0,
        }
    }

    /// One edge's score at a step with the source moving in one of the
    /// target's [`Replay::loads`] of it and held in the rest.
    pub fn score_of_one_load(&mut self, edge: usize, step: usize, load: usize) -> Option<f64> {
        let edge = self.edges[edge];
        let Head::Unit(unit) = edge.head else {
            return None;
        };
        if step == 0 {
            return Some(0.0);
        }
        let (x, z) = (edge.from, edge.to);
        let (now, before) = match edge.lag {
            Lag::Current => (self.value(x, step), self.value(x, step - 1)),
            Lag::Previous => (
                self.value(x, step - 1),
                self.value(x, step.saturating_sub(2)),
            ),
        };
        let dz = self.value(z, step) - self.value(z, step - 1);
        if dz == 0.0 || now == before {
            return Some(0.0);
        }
        // The load that moves reads a slot of its own, past the model's.
        let spare = u16::try_from(self.width).ok()?;
        let unit = &self.units[unit];
        let mut code = unit.code.clone();
        let moved = (code.code.iter_mut())
            .filter(|op| loads(op, unit, x, edge.lag))
            .nth(load)?;
        *moved = match edge.lag {
            Lag::Current => Opcode::LoadGlobalVar { off: spare },
            Lag::Previous => Opcode::LoadPrev {
                off: u16::try_from(self.width - unit.module_off).ok()?,
            },
        };
        let mut frame = self.frame(step - 1);
        frame.curr.push(now);
        frame.prev.push(now);
        if edge.lag == Lag::Previous {
            frame.at_first_step = false;
        }
        let moved = Unit {
            target: unit.target,
            reads: Vec::new(),
            code,
            context: unit.context.clone(),
            module_off: unit.module_off,
        };
        let replayed = run(&mut self.evaluator, &moved, &mut frame);
        Some((replayed - self.value(z, step - 1)) / dz.abs() * sign(now - before))
    }

    fn row(&self, step: usize) -> &[f64] {
        &self.rows[step * self.width..(step + 1) * self.width]
    }

    /// The rows `step` was computed over.
    fn frame(&self, step: usize) -> Frame {
        Frame {
            curr: self.row(step).to_vec(),
            prev: self.row(step.saturating_sub(1)).to_vec(),
            at_first_step: step == 0,
        }
    }

    /// The variable a slot belongs to: its name without its element.
    fn variable(&self, slot: usize) -> Option<&str> {
        self.name(slot)
            .map(|n| n.split_once('[').map_or(n, |(variable, _)| variable))
    }

    fn fill_reducer_nodes(&mut self) {
        for step in 0..self.n_steps() {
            let mut frame = self.frame(step);
            for &reducer in &self.reducers {
                let unit = &self.units[reducer];
                let value = run(&mut self.evaluator, unit, &mut frame);
                // A later reducer may read this one's node, at this step or
                // the next.
                self.rows[step * self.width + unit.target] = value;
            }
        }
    }
}

/// The rows of a run, apart from the rest of a [`Replay`] so that its evaluator
/// can be borrowed beside them.
#[derive(Clone, Copy)]
struct Rows<'a> {
    data: &'a [f64],
    width: usize,
}

impl Rows<'_> {
    fn value(&self, slot: usize, step: usize) -> f64 {
        self.data[step * self.width + slot]
    }
}

/// Run a unit over `frame`, and answer what it computes, leaving the frame as
/// it was but for the target's own slot.
fn run(evaluator: &mut PieceEvaluator, unit: &Unit, frame: &mut Frame) -> f64 {
    evaluator.run(
        &unit.context,
        &unit.code,
        unit.module_off,
        &mut frame.curr,
        &mut frame.prev,
        frame.at_first_step,
    );
    frame.curr[unit.target]
}

/// Whether an opcode of `unit` is a plain load of `slot`.
fn loads(op: &Opcode, unit: &Unit, slot: usize, lag: Lag) -> bool {
    match (op, lag) {
        (Opcode::LoadVar { off }, Lag::Current) | (Opcode::LoadPrev { off }, Lag::Previous) => {
            unit.module_off + *off as usize == slot
        }
        (Opcode::LoadGlobalVar { off }, Lag::Current) => *off as usize == slot,
        _ => false,
    }
}

/// `edge`'s score at `step`, over a `frame` of the step before, which is left
/// as it was.
fn changed_first(
    edge: &Edge,
    rows: Rows<'_>,
    units: &[Unit],
    stocks: &[StockUpdate],
    evaluator: &mut PieceEvaluator,
    frame: &mut Frame,
    step: usize,
) -> f64 {
    let (x, z) = (edge.from, edge.to);
    let unit = match edge.head {
        Head::Unit(unit) => &units[unit],
        Head::Stock(stock, direction) => {
            let dx = rows.value(x, step) - rows.value(x, step - 1);
            let stock = &stocks[stock];
            let row = |step: usize| &rows.data[step * rows.width..(step + 1) * rows.width];
            let dnet = stock.net(row(step)) - stock.net(row(step - 1));
            return if dx == 0.0 || dnet == 0.0 {
                0.0
            } else {
                direction * dx / dnet.abs() * sign(dx)
            };
        }
    };
    let dz = rows.value(z, step) - rows.value(z, step - 1);
    // A lagged source's own change is the one a step earlier, which the first
    // step has none of.
    let (now, before) = match edge.lag {
        Lag::Current => (rows.value(x, step), rows.value(x, step - 1)),
        Lag::Previous => (
            rows.value(x, step - 1),
            rows.value(x, step.saturating_sub(2)),
        ),
    };
    let dx = now - before;
    if dz == 0.0 || dx == 0.0 {
        return 0.0;
    }
    let side = match edge.lag {
        Lag::Current => &mut frame.curr,
        Lag::Previous => &mut frame.prev,
    };
    let held = std::mem::replace(&mut side[x], now);
    let replayed = run(evaluator, unit, frame);
    frame.curr[z] = rows.value(z, step - 1);
    let side = match edge.lag {
        Lag::Current => &mut frame.curr,
        Lag::Previous => &mut frame.prev,
    };
    side[x] = held;
    (replayed - rows.value(z, step - 1)) / dz.abs() * sign(dx)
}

/// `SIGN` as LTM's guard form has it: 0 at 0.
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn edges_of(units: &[Unit], stocks: &[StockUpdate]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        for read in &unit.reads {
            edges.push(Edge {
                from: read.slot,
                to: unit.target,
                lag: read.lag,
                head: Head::Unit(i),
            });
        }
    }
    for (i, stock) in stocks.iter().enumerate() {
        for (flows, direction) in [(&stock.inflows, 1.0), (&stock.outflows, -1.0)] {
            edges.extend(flows.iter().map(|&flow| Edge {
                from: flow,
                to: stock.target,
                lag: Lag::Current,
                head: Head::Stock(i, direction),
            }));
        }
    }
    edges
}

/// What the names of the instance at `base`, `n` slots long, begin with: its
/// shallowest name up to the last separator, an instance inside it having more.
fn instance_prefix(names: &[Option<String>], base: usize, n: usize) -> String {
    const SEPARATOR: char = '\u{B7}';
    if base == 0 {
        return String::new();
    }
    names[base..base + n]
        .iter()
        .flatten()
        .map(|name| name.split_once('[').map_or(name.as_str(), |(v, _)| v))
        .min_by_key(|name| name.matches(SEPARATOR).count())
        .and_then(|name| Some(&name[..name.rfind(SEPARATOR)? + SEPARATOR.len_utf8()]))
        .unwrap_or_default()
        .to_string()
}

/// A stretch of a flows program that leaves every stack as it found it.
enum Piece {
    /// Fills the temporary array `temp`.
    Temp {
        range: (usize, usize),
        temp: usize,
        needs: Vec<usize>,
    },
    /// Writes the slot at `off` of its instance.
    Assign {
        range: (usize, usize),
        off: usize,
        needs: Vec<usize>,
    },
    /// Runs a module instance, with the slots its input ports are bound to.
    Call { id: usize, inputs: Vec<Opcode> },
}

/// The temporary array an opcode fills.
fn temp_written(op: &Opcode) -> Option<usize> {
    match op {
        Opcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => has_write_temp.then_some(*write_temp_id as usize),
        Opcode::VectorElmMap { write_temp_id, .. }
        | Opcode::VectorSortOrder { write_temp_id }
        | Opcode::Rank { write_temp_id }
        | Opcode::LookupArray { write_temp_id, .. }
        | Opcode::AllocateAvailable { write_temp_id }
        | Opcode::AllocateByPriority { write_temp_id } => Some(*write_temp_id as usize),
        _ => None,
    }
}

/// The temporary array an opcode reads.
fn temp_read(op: &Opcode, context: &ByteCodeContext) -> Option<usize> {
    match op {
        Opcode::LoadTempConst { temp_id, .. } => Some(*temp_id as usize),
        Opcode::PushStaticView { view_id } => {
            let view = &context.static_views[*view_id as usize];
            (view.storage == ViewStorage::Temp).then_some(view.base_off as usize)
        }
        _ => None,
    }
}

/// Cut a flows program where each of its pieces ends.
fn cut(code: &[Opcode], context: &ByteCodeContext) -> Result<Vec<Piece>, String> {
    let mut pieces: Vec<Piece> = Vec::new();
    // Temps are reused from one fragment to the next, so the piece a read is
    // of is the last to have written it.
    let mut last_writer: HashMap<usize, usize> = HashMap::new();
    let (mut depth, mut views, mut iterations) = (0isize, 0isize, 0isize);
    let mut start = 0;
    let mut writing: Option<usize> = None;
    let mut needs: Vec<usize> = Vec::new();
    let end = code
        .iter()
        .position(|op| matches!(op, Opcode::Ret))
        .unwrap_or(code.len());
    for (pc, op) in code[..end].iter().enumerate() {
        let (pops, pushes) = op.stack_effect();
        depth += pushes as isize - pops as isize;
        match op {
            Opcode::PushStaticView { .. } | Opcode::PushVarViewDirect { .. } => views += 1,
            Opcode::PopView {} => views -= 1,
            Opcode::BeginIter { .. } => iterations += 1,
            Opcode::EndIter {} => iterations -= 1,
            _ => {}
        }
        // The view a `BeginIter` iterates is of the temp it fills, pushed for
        // its shape alone.
        let for_shape = matches!(code.get(pc + 1), Some(Opcode::BeginIter { .. }));
        if let Some(temp) = temp_read(op, context).filter(|_| !for_shape) {
            let Some(&writer) = last_writer.get(&temp) else {
                return Err(format!(
                    "pc {pc} reads temp {temp}, which nothing has filled"
                ));
            };
            if !needs.contains(&writer) {
                needs.push(writer);
            }
        }
        if let Some(temp) = temp_written(op) {
            if writing.is_some_and(|other| other != temp) {
                return Err(format!("pc {pc} fills a second temp in one piece"));
            }
            writing = Some(temp);
        }
        let balanced = depth == 0 && views == 0 && iterations == 0;
        let ends_statement = matches!(
            op,
            Opcode::AssignCurr { .. }
                | Opcode::AssignConstCurr { .. }
                | Opcode::BinOpAssignCurr { .. }
                | Opcode::EvalModule { .. }
        );
        if ends_statement && !balanced {
            return Err(format!("{} at pc {pc} leaves a stack in use", op.name()));
        }
        let range = (start, pc + 1);
        let piece = match op {
            Opcode::AssignCurr { off }
            | Opcode::AssignConstCurr { off, .. }
            | Opcode::BinOpAssignCurr { off, .. } => Piece::Assign {
                range,
                off: *off as usize,
                needs: std::mem::take(&mut needs),
            },
            Opcode::EvalModule { id, n_inputs } => {
                let inputs = code[start..pc].to_vec();
                if inputs.len() != *n_inputs as usize {
                    return Err(format!("module call at pc {pc}: an input is computed"));
                }
                Piece::Call {
                    id: *id as usize,
                    inputs,
                }
            }
            _ => match writing {
                Some(temp) if balanced => {
                    last_writer.insert(temp, pieces.len());
                    Piece::Temp {
                        range,
                        temp,
                        needs: std::mem::take(&mut needs),
                    }
                }
                _ => continue,
            },
        };
        pieces.push(piece);
        start = pc + 1;
        writing = None;
    }
    if start != end {
        return Err(format!("no rule for what the code from pc {start} writes"));
    }
    Ok(pieces)
}

/// Every slot a view of a chunk-shaped region addresses.
fn slots_of(view: &RuntimeView) -> Vec<usize> {
    let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; view.dims.len()];
    (0..view.size())
        .map(|_| {
            let slot = view.base_off as usize + view.flat_offset(&indices);
            increment_indices(&mut indices, &view.dims);
            slot
        })
        .collect()
}

/// One module instance being cut.
struct Scope<'a> {
    flows: &'a ByteCode,
    context: &'a Arc<ByteCodeContext>,
    pieces: &'a [Piece],
    base: usize,
    inputs: &'a [usize],
}

impl Scope<'_> {
    /// The pieces that fill the temps `roots` do and the temps those need, in
    /// program order.
    fn with_what_they_need(&self, roots: &[usize]) -> Vec<(usize, usize)> {
        let mut found: HashSet<usize> = HashSet::new();
        let mut work = roots.to_vec();
        while let Some(piece) = work.pop() {
            if found.insert(piece)
                && let Piece::Temp { needs, .. } = &self.pieces[piece]
            {
                work.extend(needs);
            }
        }
        let mut found: Vec<usize> = found.into_iter().collect();
        found.sort_unstable();
        found
            .into_iter()
            .filter_map(|piece| match &self.pieces[piece] {
                Piece::Temp { range, .. } => Some(*range),
                Piece::Assign { .. } | Piece::Call { .. } => None,
            })
            .collect()
    }

    /// The piece of `needs` that fills `temp`.
    fn writer(&self, needs: &[usize], temp: usize) -> Vec<usize> {
        needs
            .iter()
            .copied()
            .filter(|&p| matches!(&self.pieces[p], Piece::Temp { temp: t, .. } if *t == temp))
            .collect()
    }

    fn code(&self, ranges: &[(usize, usize)]) -> Vec<Opcode> {
        ranges
            .iter()
            .flat_map(|&(a, b)| self.flows.code[a..b].iter().copied())
            .collect()
    }
}

struct Cutter<'a> {
    sim: &'a CompiledSimulation,
    reducers: Reducers,
    width: usize,
    units: Vec<Unit>,
    stocks: Vec<StockUpdate>,
    reducer_units: Vec<usize>,
    /// A reducer by what it computes, so one that is written twice is one node.
    reducer_slots: HashMap<String, usize>,
    reducer_names: Vec<String>,
    ports: HashMap<usize, usize>,
    /// Each instance's base slot, length, and model.
    instances: Vec<(usize, usize, String)>,
}

impl Cutter<'_> {
    fn instance(&mut self, key: &ModuleKey, base: usize, inputs: &[usize]) -> Result<(), String> {
        let sim = self.sim;
        let module: &CompiledModule = sim
            .modules
            .get(key)
            .ok_or_else(|| format!("no module {}", key.0.as_str()))?;
        self.instances
            .push((base, module.n_slots, key.0.as_str().to_string()));
        let in_module = |e: String| format!("{}: {e}", key.0.as_str());
        let pieces = cut(&module.compiled_flows.code, &module.context).map_err(in_module)?;
        let scope = Scope {
            flows: &module.compiled_flows,
            context: &module.context,
            pieces: &pieces,
            base,
            inputs,
        };
        for piece in &pieces {
            match piece {
                Piece::Temp { .. } => {}
                Piece::Assign { range, off, needs } => {
                    if let [Opcode::LoadModuleInput { input }, Opcode::AssignCurr { .. }] =
                        scope.flows.code[range.0..range.1]
                    {
                        self.ports.insert(base + off, inputs[input as usize]);
                    }
                    self.assign(&scope, *range, base + off, needs)
                        .map_err(in_module)?;
                }
                Piece::Call {
                    id,
                    inputs: arguments,
                } => {
                    let bound = arguments
                        .iter()
                        .map(|argument| match argument {
                            Opcode::LoadVar { off } => Ok(base + *off as usize),
                            Opcode::LoadGlobalVar { off } => Ok(*off as usize),
                            Opcode::LoadModuleInput { input } => Ok(inputs[*input as usize]),
                            other => Err(format!("a module input is a {}", other.name())),
                        })
                        .collect::<Result<Vec<usize>, String>>()
                        .map_err(in_module)?;
                    let declared = &module.context.modules[*id];
                    let child = make_module_key(&declared.model_name, &declared.input_set);
                    self.instance(&child, base + declared.off, &bound)?;
                }
            }
        }
        self.stock_updates(&module.compiled_stocks, base, inputs)
            .map_err(in_module)
    }

    /// The unit of one assignment, its reducers taken out of it when they are
    /// nodes.
    fn assign(
        &mut self,
        scope: &Scope<'_>,
        range: (usize, usize),
        target: usize,
        needs: &[usize],
    ) -> Result<(), String> {
        let body = &scope.flows.code[range.0..range.1];
        let mut kept: Vec<Opcode> = Vec::with_capacity(body.len());
        let mut at = 0;
        while at < body.len() {
            if let Some(slot) = self.reducer_node(scope, body, at, needs)? {
                kept.push(Opcode::LoadGlobalVar {
                    off: u16::try_from(slot).map_err(|_| "no slot left for a reducer node")?,
                });
                at += 3;
            } else {
                kept.push(body[at]);
                at += 1;
            }
        }
        // A reducer that became a node takes the temps only it read with it.
        let mut still: Vec<usize> = Vec::new();
        for temp in kept.iter().filter_map(|op| temp_read(op, scope.context)) {
            still.extend(scope.writer(needs, temp));
        }
        let mut code = scope.code(&scope.with_what_they_need(&still));
        code.extend(kept);
        self.unit(scope, target, code)
    }

    /// The node of the reducer `body` holds at `at`, or `None` where it holds
    /// none.
    fn reducer_node(
        &mut self,
        scope: &Scope<'_>,
        body: &[Opcode],
        at: usize,
        needs: &[usize],
    ) -> Result<Option<usize>, String> {
        if self.reducers == Reducers::Inline {
            return Ok(None);
        }
        let Some(
            [
                Opcode::PushStaticView { view_id },
                reducer,
                Opcode::PopView {},
            ],
        ) = body.get(at..at + 3)
        else {
            return Ok(None);
        };
        let name = match reducer {
            Opcode::ArraySum {} => "sum",
            Opcode::ArrayMax {} => "max",
            Opcode::ArrayMin {} => "min",
            Opcode::ArrayMean {} => "mean",
            Opcode::ArrayStddev {} => "stddev",
            _ => return Ok(None),
        };
        let view = &scope.context.static_views[*view_id as usize];
        let blocks = match view.storage {
            ViewStorage::Curr => Vec::new(),
            ViewStorage::Temp => {
                scope.with_what_they_need(&scope.writer(needs, view.base_off as usize))
            }
            // A reducer of a snapshot is no function of this step's inputs.
            ViewStorage::Prev | ViewStorage::Initial => return Ok(None),
        };
        let key = format!(
            "{name} {:p} {} {} {}",
            Arc::as_ptr(scope.context),
            scope.base,
            view_key(view),
            blocks
                .iter()
                .map(|&(a, b)| block_key(&scope.flows.code[a..b], scope))
                .collect::<String>(),
        );
        if let Some(&slot) = self.reducer_slots.get(&key) {
            return Ok(Some(slot));
        }
        let slot = self.width;
        self.width += 1;
        self.reducer_names
            .push(format!("<{name} {}>", self.reducer_names.len()));
        self.reducer_slots.insert(key, slot);
        let mut code = scope.code(&blocks);
        code.extend_from_slice(&body[at..at + 3]);
        code.push(Opcode::AssignCurr {
            off: u16::try_from(slot - scope.base).map_err(|_| "no slot left for a reducer node")?,
        });
        self.reducer_units.push(self.units.len());
        self.unit(scope, slot, code)?;
        Ok(Some(slot))
    }

    /// Add the unit of `code`, which computes `target`.
    fn unit(
        &mut self,
        scope: &Scope<'_>,
        target: usize,
        mut code: Vec<Opcode>,
    ) -> Result<(), String> {
        let base = scope.base;
        let mut reads: Vec<Read> = Vec::new();
        let mut literals: Vec<f64> = Vec::new();
        let mut read = |slot: usize, lag: Lag| {
            // The clock and its kin are not variables: nothing is a link from
            // them.
            if slot >= IMPLICIT_VAR_COUNT {
                reads.push(Read { slot, lag });
            }
        };
        // The code's own literals, so a unit does not carry its module's pool.
        let mut literal = |id: &mut u16| {
            literals.push(scope.flows.literals[*id as usize]);
            *id = (literals.len() - 1) as u16;
        };
        let mut subscript_extent = 1usize;
        let mut views: Vec<Option<StaticArrayView>> = Vec::new();
        for op in &mut code {
            match op {
                Opcode::LoadConstant { id } => literal(id),
                Opcode::AssignConstCurr { literal_id, .. } => literal(literal_id),

                Opcode::LoadVar { off } => read(base + *off as usize, Lag::Current),
                Opcode::LoadGlobalVar { off } => read(*off as usize, Lag::Current),
                Opcode::LoadPrev { off } => read(base + *off as usize, Lag::Previous),
                Opcode::LoadModuleInput { input } => {
                    let slot = scope.inputs[*input as usize];
                    read(slot, Lag::Current);
                    // A piece runs without the call that would pass it.
                    *op = Opcode::LoadGlobalVar { off: slot as u16 };
                }

                Opcode::PushSubscriptIndex { bounds } => subscript_extent *= *bounds as usize,
                Opcode::LoadSubscript { off } => {
                    let first = base + *off as usize;
                    (first..first + subscript_extent).for_each(|slot| read(slot, Lag::Current));
                    subscript_extent = 1;
                }

                Opcode::PushStaticView { view_id } => {
                    let view = &scope.context.static_views[*view_id as usize];
                    if let Some(lag) = lag_of(view.storage) {
                        slots_of(&view.to_runtime_view(base as u32))
                            .into_iter()
                            .for_each(|slot| read(slot, lag));
                    }
                    views.push(Some(view.clone()));
                }
                Opcode::PushVarViewDirect {
                    base_off,
                    dim_list_id,
                } => {
                    let (n, dims) = scope.context.get_dim_list(*dim_list_id);
                    let dims: SmallVec<[u16; 4]> = dims[..n as usize].iter().copied().collect();
                    let ids: SmallVec<[DimId; 4]> = dims.iter().map(|_| 0).collect();
                    slots_of(&RuntimeView::for_var(
                        (base + *base_off as usize) as u32,
                        dims,
                        ids,
                    ))
                    .into_iter()
                    .for_each(|slot| read(slot, Lag::Current));
                    views.push(None);
                }
                Opcode::PopView {} => {
                    views.pop();
                }
                // Reads its source variable past the view that names it: the
                // offsets are from the view's first element, over the whole
                // variable.
                Opcode::VectorElmMap {
                    full_source_len, ..
                } => {
                    if let [.., Some(source), _offsets] = views.as_slice()
                        && let Some(lag) = lag_of(source.storage)
                    {
                        let first = base + source.base_off as usize;
                        (first..first + *full_source_len as usize).for_each(|slot| read(slot, lag));
                    }
                }

                // These read the stacks, the temps, the tables and the initial
                // values: nothing a step can change.
                Opcode::Op2 { .. }
                | Opcode::Not {}
                | Opcode::LoadInitial { .. }
                | Opcode::SetCond {}
                | Opcode::If {}
                | Opcode::AssignCurr { .. }
                | Opcode::BinOpAssignCurr { .. }
                | Opcode::Apply { .. }
                | Opcode::Lookup { .. }
                | Opcode::LookupDirect { .. }
                | Opcode::LookupArray { .. }
                | Opcode::ViewSubscriptDynamic { .. }
                | Opcode::ViewRangeDynamic { .. }
                | Opcode::LoadTempConst { .. }
                | Opcode::BeginIter { .. }
                | Opcode::LoadIterViewAt { .. }
                | Opcode::StoreIterElement {}
                | Opcode::NextIterOrJump { .. }
                | Opcode::EndIter {}
                | Opcode::ArraySum {}
                | Opcode::ArrayMax {}
                | Opcode::ArrayMin {}
                | Opcode::ArrayMean {}
                | Opcode::ArrayStddev {}
                | Opcode::ArraySize {}
                | Opcode::VectorSelect {}
                | Opcode::VectorSortOrder { .. }
                | Opcode::Rank { .. }
                | Opcode::AllocateAvailable { .. }
                | Opcode::AllocateByPriority { .. } => {}

                other => return Err(format!("no rule for what {} reads", other.name())),
            }
        }
        reads.sort_unstable();
        reads.dedup();
        self.units.push(Unit {
            target,
            reads,
            code: ByteCode { literals, code },
            context: scope.context.clone(),
            module_off: base,
        });
        Ok(())
    }

    /// Read each stock's update as a sum of the flows it adds and takes away.
    fn stock_updates(
        &mut self,
        stocks: &ByteCode,
        base: usize,
        inputs: &[usize],
    ) -> Result<(), String> {
        #[derive(Clone)]
        enum Term {
            Zero,
            Slots(Vec<usize>),
            Net(Vec<usize>, Vec<usize>),
            Dt,
            Change(Vec<usize>, Vec<usize>),
        }
        fn slots(term: Term) -> Option<Vec<usize>> {
            match term {
                Term::Zero => Some(Vec::new()),
                Term::Slots(slots) => Some(slots),
                Term::Net(..) | Term::Dt | Term::Change(..) => None,
            }
        }
        let mut stack: Vec<Term> = Vec::new();
        for op in &stocks.code {
            let not_a_sum = || format!("a stock's update is not a sum of flows ({})", op.name());
            let term = match op {
                Opcode::LoadVar { off } => Term::Slots(vec![base + *off as usize]),
                Opcode::LoadModuleInput { input } => Term::Slots(vec![inputs[*input as usize]]),
                Opcode::LoadGlobalVar { off } if *off as usize == DT_OFF => Term::Dt,
                Opcode::LoadConstant { id } if stocks.literals[*id as usize] == 0.0 => Term::Zero,
                Opcode::Op2 { op: operator } => {
                    let (r, l) = (stack.pop(), stack.pop());
                    let (Some(l), Some(r)) = (l, r) else {
                        return Err(not_a_sum());
                    };
                    match (operator, l, r) {
                        (Op2::Add, l, r) => {
                            let (l, r) = slots(l).zip(slots(r)).ok_or_else(not_a_sum)?;
                            Term::Slots(l.into_iter().chain(r).collect())
                        }
                        (Op2::Sub, l, r) => {
                            let (l, r) = slots(l).zip(slots(r)).ok_or_else(not_a_sum)?;
                            Term::Net(l, r)
                        }
                        (Op2::Mul, Term::Net(inflows, outflows), Term::Dt) => {
                            Term::Change(inflows, outflows)
                        }
                        // A stock with no flows: the empty net is folded.
                        (Op2::Mul, Term::Zero, Term::Dt) => Term::Change(Vec::new(), Vec::new()),
                        _ => return Err(not_a_sum()),
                    }
                }
                Opcode::BinOpAssignNext { op: Op2::Add, off } => {
                    let target = base + *off as usize;
                    match (stack.pop(), stack.pop()) {
                        (Some(Term::Change(inflows, outflows)), Some(Term::Slots(stock)))
                            if stock == [target] =>
                        {
                            self.stocks.push(StockUpdate {
                                target,
                                inflows,
                                outflows,
                            });
                        }
                        _ => return Err(not_a_sum()),
                    }
                    continue;
                }
                // The instance's own stocks are cut with the instance.
                Opcode::EvalModule { .. } => {
                    stack.clear();
                    continue;
                }
                Opcode::Ret => break,
                _ => return Err(not_a_sum()),
            };
            stack.push(term);
        }
        Ok(())
    }
}

fn lag_of(storage: ViewStorage) -> Option<Lag> {
    match storage {
        ViewStorage::Curr => Some(Lag::Current),
        ViewStorage::Prev => Some(Lag::Previous),
        ViewStorage::Temp | ViewStorage::Initial => None,
    }
}

/// What a view addresses, whichever table it is in. A temp's id is not part of
/// it: one array value is given one id here and another there.
fn view_key(view: &StaticArrayView) -> String {
    let base = match view.storage {
        ViewStorage::Temp => None,
        ViewStorage::Curr | ViewStorage::Prev | ViewStorage::Initial => Some(view.base_off),
    };
    format!(
        "{:?} {base:?} {:?} {:?} {} {:?};",
        view.storage, view.dims, view.strides, view.offset, view.sparse
    )
}

/// What a piece that fills a temp computes, with the ids of its literals, views
/// and temps replaced by what they stand for.
fn block_key(code: &[Opcode], scope: &Scope<'_>) -> String {
    code.iter()
        .map(|op| match op {
            Opcode::LoadConstant { id } => {
                format!("{:x};", scope.flows.literals[*id as usize].to_bits())
            }
            Opcode::PushStaticView { view_id } => {
                view_key(&scope.context.static_views[*view_id as usize])
            }
            Opcode::LoadTempConst { index, .. } => format!("temp {index};"),
            other if temp_written(other).is_some() => format!("{};", other.name()),
            other => format!("{other:?};"),
        })
        .collect()
}

#[cfg(test)]
#[path = "ltm_replay_tests.rs"]
mod tests;
