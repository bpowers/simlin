// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::rc::Rc;

use float_cmp::approx_eq;
use smallvec::SmallVec;

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledModule, ModuleId, Op2, Opcode,
};
use crate::common::{Ident, Result};
use crate::datamodel::{Dimension, Dt, SimMethod, SimSpecs};
use crate::sim_err;

pub(crate) const TIME_OFF: usize = 0;
pub(crate) const DT_OFF: usize = 1;
pub(crate) const INITIAL_TIME_OFF: usize = 2;
pub(crate) const FINAL_TIME_OFF: usize = 3;
pub(crate) const IMPLICIT_VAR_COUNT: usize = 4;

pub(crate) fn is_truthy(n: f64) -> bool {
    let is_false = approx_eq!(f64, n, 0.0);
    !is_false
}

#[derive(Clone, Debug)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<Ident, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: String,
    pub(crate) offsets: HashMap<Ident, usize>,
}

#[derive(Clone, Debug)]
struct CompiledSlicedSimulation {
    initial_modules: HashMap<Ident, CompiledModuleSlice>,
    flow_modules: HashMap<Ident, CompiledModuleSlice>,
    stock_modules: HashMap<Ident, CompiledModuleSlice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepPart {
    Initials,
    Flows,
    Stocks,
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum Method {
    Euler,
}

#[derive(Clone, Debug)]
pub struct Specs {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    pub save_step: f64,
    pub method: Method,
}

impl Specs {
    pub fn from(specs: &SimSpecs) -> Self {
        let dt: f64 = match &specs.dt {
            Dt::Dt(value) => *value,
            Dt::Reciprocal(value) => 1.0 / *value,
        };

        let save_step: f64 = match &specs.save_step {
            None => dt,
            Some(save_step) => match save_step {
                Dt::Dt(value) => *value,
                Dt::Reciprocal(value) => 1.0 / *value,
            },
        };

        let method = match specs.sim_method {
            SimMethod::Euler => Method::Euler,
            SimMethod::RungeKutta4 => {
                eprintln!("warning, simulation requested 'rk4', but only support Euler");
                Method::Euler
            }
        };

        Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            save_step,
            method,
        }
    }
}

#[derive(Debug)]
pub struct Results {
    pub offsets: HashMap<String, usize>,
    // one large allocation
    pub data: Box<[f64]>,
    pub step_size: usize,
    pub step_count: usize,
    pub specs: Specs,
    pub is_vensim: bool,
}

impl Results {
    pub fn print_tsv(&self) {
        self.print_tsv_comparison(None)
    }
    pub fn print_tsv_comparison(&self, reference: Option<&Results>) {
        let var_names = {
            let offset_name_map: HashMap<usize, &str> =
                self.offsets.iter().map(|(k, v)| (*v, k.as_str())).collect();
            let mut var_names: Vec<&str> = Vec::with_capacity(self.step_size);
            for i in 0..(self.step_size) {
                let name = if offset_name_map.contains_key(&i) {
                    offset_name_map[&i]
                } else {
                    "UNKNOWN"
                };
                var_names.push(name);
            }
            var_names
        };

        if reference.is_some() {
            print!("series\t");
        }

        // print header
        for (i, id) in var_names.iter().enumerate() {
            print!("{}", id);
            if i == var_names.len() - 1 {
                println!();
            } else {
                print!("\t");
            }
        }

        match reference {
            Some(reference) => {
                for (curr, ref_curr) in self.iter().zip(reference.iter()) {
                    if curr[TIME_OFF] > self.specs.stop {
                        break;
                    }
                    print!("reference\t");
                    for (i, _) in curr.iter().enumerate() {
                        let var_name = var_names[i];
                        if let Some(off) = reference.offsets.get(var_name) {
                            let val = ref_curr[*off];
                            print!("{}", val);
                        } else {
                            print!("")
                        }
                        if i == var_names.len() - 1 {
                            println!();
                        } else {
                            print!("\t");
                        }
                    }
                    print!("simlin\t");
                    for (i, val) in curr.iter().enumerate() {
                        print!("{}", val);
                        if i == var_names.len() - 1 {
                            println!();
                        } else {
                            print!("\t");
                        }
                    }
                }
            }
            None => {
                for curr in self.iter() {
                    if curr[TIME_OFF] > self.specs.stop {
                        break;
                    }
                    for (i, val) in curr.iter().enumerate() {
                        print!("{}", val);
                        if i == var_names.len() - 1 {
                            println!();
                        } else {
                            print!("\t");
                        }
                    }
                }
            }
        }
    }

    pub fn iter(&self) -> std::iter::Take<std::slice::Chunks<f64>> {
        self.data.chunks(self.step_size).take(self.step_count)
    }
}

#[derive(Clone, Debug)]
pub struct Vm {
    specs: Specs,
    root: Ident,
    offsets: HashMap<Ident, usize>,
    sliced_sim: CompiledSlicedSimulation,
    n_slots: usize,
    n_chunks: usize,
    data: Option<Box<[f64]>>,
}

#[derive(Debug)]
struct Stack {
    stack: Vec<f64>,
}

impl Stack {
    fn new() -> Self {
        Stack {
            stack: Vec::with_capacity(32),
        }
    }

    #[inline(always)]
    fn push(&mut self, value: f64) {
        self.stack.push(value)
    }

    #[inline(always)]
    fn pop(&mut self) -> f64 {
        self.stack.pop().unwrap()
    }
}

#[derive(Clone, Debug)]
struct CompiledModuleSlice {
    #[allow(dead_code)]
    ident: Ident,
    context: Rc<ByteCodeContext>,
    bytecode: Rc<ByteCode>,
    part: StepPart,
}

impl CompiledModuleSlice {
    fn new(module: &CompiledModule, part: StepPart) -> Self {
        CompiledModuleSlice {
            ident: module.ident.clone(),
            context: module.context.clone(),
            bytecode: match part {
                StepPart::Initials => module.compiled_initials.clone(),
                StepPart::Flows => module.compiled_flows.clone(),
                StepPart::Stocks => module.compiled_stocks.clone(),
            },
            part,
        }
    }
}

impl Vm {
    pub fn new(sim: CompiledSimulation) -> Result<Vm> {
        if sim.specs.stop < sim.specs.start {
            return sim_err!(
                BadSimSpecs,
                "end time has to be after start time".to_string()
            );
        }
        if approx_eq!(f64, sim.specs.dt, 0.0) {
            return sim_err!(BadSimSpecs, "dt must be greater than 0".to_string());
        }

        let save_step = if sim.specs.save_step > sim.specs.dt {
            sim.specs.save_step
        } else {
            sim.specs.dt
        };
        let n_slots = sim.modules[&sim.root].n_slots;
        let n_chunks: usize = ((sim.specs.stop - sim.specs.start) / save_step + 1.0) as usize;
        let data: Box<[f64]> = vec![0.0; n_slots * (n_chunks + 2)].into_boxed_slice();
        Ok(Vm {
            specs: sim.specs,
            root: sim.root,
            offsets: sim.offsets,
            sliced_sim: CompiledSlicedSimulation {
                initial_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.clone(), CompiledModuleSlice::new(m, StepPart::Initials)))
                    .collect(),
                flow_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.clone(), CompiledModuleSlice::new(m, StepPart::Flows)))
                    .collect(),
                stock_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.clone(), CompiledModuleSlice::new(m, StepPart::Stocks)))
                    .collect(),
            },
            n_slots,
            n_chunks,
            data: Some(data),
        })
    }

    pub fn run_to_end(&mut self) -> Result<()> {
        let end = self.specs.stop;
        self.run_to(end)
    }

    #[inline(never)]
    pub fn run_to(&mut self, end: f64) -> Result<()> {
        let spec = &self.specs;

        let sliced_sim = &self.sliced_sim;
        let module_initials = &sliced_sim.initial_modules[&self.root];
        let module_flows = &sliced_sim.flow_modules[&self.root];
        let module_stocks = &sliced_sim.stock_modules[&self.root];

        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5).floor() as usize);

        let dt = spec.dt;

        let mut data = None;
        std::mem::swap(&mut data, &mut self.data);
        let mut data = data.unwrap();

        {
            let mut stack = Stack::new();
            let module_inputs: &[f64] = &[0.0; 0];

            let mut slabs = data.chunks_mut(self.n_slots);
            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = spec.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = spec.start;
            curr[FINAL_TIME_OFF] = spec.stop;
            self.eval(module_initials, 0, module_inputs, curr, next, &mut stack);
            let mut is_initial_timestep = true;
            let mut step = 0;
            while curr[TIME_OFF] <= end {
                self.eval(module_flows, 0, module_inputs, curr, next, &mut stack);
                self.eval(module_stocks, 0, module_inputs, curr, next, &mut stack);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                next[DT_OFF] = curr[DT_OFF];
                next[INITIAL_TIME_OFF] = curr[INITIAL_TIME_OFF];
                next[FINAL_TIME_OFF] = curr[FINAL_TIME_OFF];
                step += 1;
                if step != save_every && !is_initial_timestep {
                    let curr = curr.borrow_mut();
                    curr.copy_from_slice(next);
                } else {
                    curr = next;
                    let maybe_next = slabs.next();
                    if maybe_next.is_none() {
                        break;
                    }
                    next = maybe_next.unwrap();
                    step = 0;
                    is_initial_timestep = false;
                }
            }
            // ensure we've calculated stock + flow values for the dt <= end_time
            assert!(curr[TIME_OFF] > end);
        }

        let mut data = Some(data);
        std::mem::swap(&mut data, &mut self.data);

        Ok(())
    }

    pub fn into_results(self) -> Results {
        Results {
            offsets: self.offsets.clone(),
            data: self.data.unwrap(),
            step_size: self.n_slots,
            step_count: self.n_chunks,
            specs: self.specs,
            is_vensim: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn eval_module(
        &self,
        parent_module: &CompiledModuleSlice,
        parent_module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        id: ModuleId,
    ) {
        let new_module_decl = &parent_module.context.modules[id as usize];
        let model_name = new_module_decl.model_name.as_str();
        let sliced_sim = &self.sliced_sim;
        let module = match parent_module.part {
            StepPart::Initials => &sliced_sim.initial_modules[model_name],
            StepPart::Flows => &sliced_sim.flow_modules[model_name],
            StepPart::Stocks => &sliced_sim.stock_modules[model_name],
        };

        let module_off = parent_module_off + new_module_decl.off;
        self.eval(module, module_off, module_inputs, curr, next, stack);
    }

    fn eval(
        &self,
        module: &CompiledModuleSlice,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
    ) {
        let bytecode = &module.bytecode;

        let mut condition = false;
        let mut subscript_index: Vec<(u16, u16)> = vec![];
        let mut subscript_index_valid = true;

        let code = &bytecode.code;
        for op in code.iter() {
            match *op {
                Opcode::Op2 { op } => {
                    let r = stack.pop();
                    let l = stack.pop();
                    let result = match op {
                        Op2::Add => l + r,
                        Op2::Sub => l - r,
                        Op2::Exp => l.powf(r),
                        Op2::Mul => l * r,
                        Op2::Div => l / r,
                        Op2::Mod => l.rem_euclid(r),
                        Op2::Gt => (l > r) as i8 as f64,
                        Op2::Gte => (l >= r) as i8 as f64,
                        Op2::Lt => (l < r) as i8 as f64,
                        Op2::Lte => (l <= r) as i8 as f64,
                        Op2::Eq => approx_eq!(f64, l, r) as i8 as f64,
                        Op2::And => (is_truthy(l) && is_truthy(r)) as i8 as f64,
                        Op2::Or => (is_truthy(l) || is_truthy(r)) as i8 as f64,
                    };
                    stack.push(result);
                }
                Opcode::Not {} => {
                    let r = stack.pop();
                    stack.push((!is_truthy(r)) as i8 as f64);
                }
                Opcode::LoadConstant { id } => {
                    stack.push(bytecode.literals[id as usize]);
                }
                Opcode::LoadGlobalVar { off } => {
                    stack.push(curr[off as usize]);
                }
                Opcode::LoadVar { off } => {
                    stack.push(curr[module_off + off as usize]);
                }
                Opcode::PushSubscriptIndex { bounds } => {
                    let index = stack.pop().floor() as u16;
                    if index == 0 || index > bounds {
                        subscript_index_valid = false;
                    } else {
                        // we convert from 1-based to 0-based here
                        subscript_index.push((index - 1, bounds));
                        subscript_index_valid &= true;
                    };
                }
                Opcode::LoadSubscript { off } => {
                    let result = if subscript_index_valid {
                        // the subscript index is 1-based, but curr is 0-based.
                        let mut index = 0;
                        for (i, bounds) in subscript_index.iter() {
                            index *= *bounds as usize;
                            index += *i as usize;
                        }
                        curr[module_off + off as usize + index]
                    } else {
                        f64::NAN
                    };
                    stack.push(result);
                    subscript_index.clear();
                    subscript_index_valid = true;
                }
                Opcode::SetCond {} => {
                    condition = is_truthy(stack.pop());
                }
                Opcode::If {} => {
                    let f = stack.pop();
                    let t = stack.pop();
                    let result = if condition { t } else { f };
                    stack.push(result);
                }
                Opcode::LoadModuleInput { input } => {
                    stack.push(module_inputs[input as usize]);
                }
                Opcode::EvalModule { id, n_inputs } => {
                    use std::iter;
                    let mut module_inputs: SmallVec<[f64; 16]> =
                        iter::repeat(0.0).take(n_inputs as usize).collect();
                    for j in (0..(n_inputs as usize)).rev() {
                        module_inputs[j] = stack.pop();
                    }
                    self.eval_module(module, module_off, &module_inputs, curr, next, stack, id);
                }
                Opcode::AssignCurr { off } => {
                    curr[module_off + off as usize] = stack.pop();
                    assert_eq!(0, stack.stack.len());
                }
                Opcode::AssignNext { off } => {
                    next[module_off + off as usize] = stack.pop();
                    assert_eq!(0, stack.stack.len());
                }
                Opcode::Apply { func } => {
                    let time = curr[TIME_OFF];
                    let dt = curr[DT_OFF];
                    let c = stack.pop();
                    let b = stack.pop();
                    let a = stack.pop();

                    stack.push(apply(func, time, dt, a, b, c));
                }
                Opcode::Lookup { gf } => {
                    let index = stack.pop();
                    let gf = &module.context.graphical_functions[gf as usize];
                    stack.push(lookup(gf, index));
                }
                Opcode::Ret => {
                    break;
                }
            }
        }
    }

    #[cfg(test)]
    pub fn debug_print_bytecode(&self, _model_name: &str) {
        let mut model_names: Vec<_> = self.sliced_sim.initial_modules.keys().collect();
        model_names.sort_unstable();
        for model_name in model_names {
            eprintln!("\n\nCOMPILED MODEL: {}", model_name);

            let initial_bc = &self.sliced_sim.initial_modules[model_name].bytecode;
            let flows_bc = &self.sliced_sim.flow_modules[model_name].bytecode;
            let stocks_bc = &self.sliced_sim.stock_modules[model_name].bytecode;

            eprintln!("\ninitial literals:");
            for (i, lit) in initial_bc.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\ninital bytecode:");
            for op in initial_bc.code.iter() {
                eprintln!("\t{:?}", op);
            }

            eprintln!("\nflows literals:");
            for (i, lit) in flows_bc.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\nflows bytecode:");
            for op in flows_bc.code.iter() {
                eprintln!("\t{:?}", op);
            }

            eprintln!("\nstocks literals:");
            for (i, lit) in stocks_bc.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\nstocks bytecode:");
            for op in stocks_bc.code.iter() {
                eprintln!("\t{:?}", op);
            }
        }
    }
}

#[inline(always)]
fn apply(func: BuiltinId, time: f64, dt: f64, a: f64, b: f64, c: f64) -> f64 {
    match func {
        BuiltinId::Abs => a.abs(),
        BuiltinId::Arccos => a.acos(),
        BuiltinId::Arcsin => a.asin(),
        BuiltinId::Arctan => a.atan(),
        BuiltinId::Cos => a.cos(),
        BuiltinId::Exp => a.exp(),
        BuiltinId::Inf => std::f64::INFINITY,
        BuiltinId::Int => a.floor(),
        BuiltinId::Ln => a.ln(),
        BuiltinId::Log10 => a.log10(),
        BuiltinId::Max => {
            if a > b {
                a
            } else {
                b
            }
        }
        BuiltinId::Min => {
            if a < b {
                a
            } else {
                b
            }
        }
        BuiltinId::Pi => std::f64::consts::PI,
        BuiltinId::Pulse => {
            let volume = a;
            let first_pulse = b;
            let interval = c;
            pulse(time, dt, volume, first_pulse, interval)
        }
        BuiltinId::Ramp => {
            let slope = a;
            let start_time = b;
            let end_time = c;
            ramp(time, slope, start_time, Some(end_time))
        }
        BuiltinId::SafeDiv => {
            if b != 0.0 {
                a / b
            } else {
                c
            }
        }
        BuiltinId::Sin => a.sin(),
        BuiltinId::Sqrt => a.sqrt(),
        BuiltinId::Step => {
            let height = a;
            let step_time = b;
            step(time, dt, height, step_time)
        }
        BuiltinId::Tan => a.tan(),
    }
}

pub(crate) fn ramp(time: f64, slope: f64, start_time: f64, end_time: Option<f64>) -> f64 {
    if time > start_time {
        let done_ramping = end_time.is_some() && time >= end_time.unwrap();
        if done_ramping {
            slope * (end_time.unwrap() - start_time)
        } else {
            slope * (time - start_time)
        }
    } else {
        0.0
    }
}

pub(crate) fn step(time: f64, dt: f64, height: f64, step_time: f64) -> f64 {
    if time + dt / 2.0 > step_time {
        height
    } else {
        0.0
    }
}

#[inline(never)]
pub(crate) fn pulse(time: f64, dt: f64, volume: f64, first_pulse: f64, interval: f64) -> f64 {
    if time < first_pulse {
        return 0.0;
    }

    let mut next_pulse = first_pulse;
    while time >= next_pulse {
        if time < next_pulse + dt {
            return volume / dt;
        } else if interval <= 0.0 {
            break;
        } else {
            next_pulse += interval;
        }
    }

    0.0
}

pub struct SubscriptOffsetIterator {
    n: usize,
    size: usize,
    lengths: Vec<usize>,
    next: Vec<usize>,
}

impl SubscriptOffsetIterator {
    pub fn new(arrays: &[Dimension]) -> Self {
        SubscriptOffsetIterator {
            n: 0,
            size: arrays.iter().map(|v| v.len()).product(),
            lengths: arrays.iter().map(|v| v.len()).collect(),
            next: vec![0; arrays.len()],
        }
    }
}

impl Iterator for SubscriptOffsetIterator {
    type Item = Vec<usize>;

    fn next(&mut self) -> Option<Vec<usize>> {
        if self.n >= self.size {
            return None;
        }

        let curr = self.next.clone();

        assert_eq!(self.lengths.len(), self.next.len());

        let mut carry = 1_usize;
        for (i, n) in self.next.iter_mut().enumerate().rev() {
            let orig_n = *n;
            let orig_carry = carry;
            *n = (*n + carry) % self.lengths[i];
            carry = ((orig_n != 0 && *n == 0) || (orig_carry == 1 && self.lengths[i] < 2)) as usize;
        }

        self.n += 1;

        Some(curr)
    }
}

#[test]
fn test_subscript_offset_iter() {
    let empty_dim = Dimension::Named("".to_string(), vec![]);
    let one_dim = Dimension::Named("".to_string(), vec!["0".to_owned()]);
    let two_dim = Dimension::Named("".to_string(), vec!["0".to_owned(), "1".to_owned()]);
    let three_dim = Dimension::Named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    );
    let cases: &[(Vec<Dimension>, Vec<Vec<usize>>)] = &[
        (vec![empty_dim.clone()], vec![]),
        (vec![empty_dim.clone(), empty_dim], vec![]),
        (vec![three_dim.clone()], vec![vec![0], vec![1], vec![2]]),
        (
            vec![three_dim.clone(), two_dim.clone()],
            vec![
                vec![0, 0],
                vec![0, 1],
                vec![1, 0],
                vec![1, 1],
                vec![2, 0],
                vec![2, 1],
            ],
        ),
        (
            vec![three_dim, one_dim, two_dim],
            vec![
                vec![0, 0, 0],
                vec![0, 0, 1],
                vec![1, 0, 0],
                vec![1, 0, 1],
                vec![2, 0, 0],
                vec![2, 0, 1],
            ],
        ),
    ];

    for (input, expected) in cases {
        let mut n = 0;
        for (i, subscripts) in SubscriptOffsetIterator::new(input).enumerate() {
            assert_eq!(expected[i], subscripts);
            n += 1;
        }
        assert_eq!(expected.len(), n);
    }
}

pub struct SubscriptIterator<'a> {
    dims: &'a [Dimension],
    offsets: SubscriptOffsetIterator,
}

impl<'a> SubscriptIterator<'a> {
    pub fn new(dims: &'a [Dimension]) -> Self {
        SubscriptIterator {
            dims,
            offsets: SubscriptOffsetIterator::new(dims),
        }
    }
}

impl<'a> Iterator for SubscriptIterator<'a> {
    type Item = Vec<&'a str>;

    fn next(&mut self) -> Option<Vec<&'a str>> {
        self.offsets.next().map(|subscripts| {
            subscripts
                .iter()
                .enumerate()
                .map(|(i, elem)| {
                    if let Dimension::Named(_, elements) = &self.dims[i] {
                        elements[*elem].as_str()
                    } else {
                        unreachable!("expected a named dimension")
                    }
                })
                .collect()
        })
    }
}

#[test]
fn test_subscript_iter() {
    let empty_dim = Dimension::Named("".to_string(), vec![]);
    let one_dim = Dimension::Named("".to_string(), vec!["0".to_owned()]);
    let two_dim = Dimension::Named("".to_string(), vec!["0".to_owned(), "1".to_owned()]);
    let three_dim = Dimension::Named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    );
    let cases: &[(Vec<Dimension>, Vec<Vec<&str>>)] = &[
        (vec![empty_dim.clone()], vec![]),
        (vec![empty_dim.clone(), empty_dim], vec![]),
        (
            vec![three_dim.clone()],
            vec![vec!["0"], vec!["1"], vec!["2"]],
        ),
        (
            vec![three_dim.clone(), two_dim.clone()],
            vec![
                vec!["0", "0"],
                vec!["0", "1"],
                vec!["1", "0"],
                vec!["1", "1"],
                vec!["2", "0"],
                vec!["2", "1"],
            ],
        ),
        (
            vec![three_dim, one_dim, two_dim],
            vec![
                vec!["0", "0", "0"],
                vec!["0", "0", "1"],
                vec!["1", "0", "0"],
                vec!["1", "0", "1"],
                vec!["2", "0", "0"],
                vec!["2", "0", "1"],
            ],
        ),
    ];

    for (input, expected) in cases {
        for (i, subscripts) in SubscriptIterator::new(input).enumerate() {
            eprintln!("exp: {:?}", expected[i]);
            eprintln!("got: {:?}", subscripts);
            assert_eq!(expected[i], subscripts);
        }
    }
}

#[inline(never)]
fn lookup(table: &[(f64, f64)], index: f64) -> f64 {
    if table.is_empty() {
        return f64::NAN;
    }

    if index.is_nan() {
        // things get wonky below if we try to binary search for NaN
        return f64::NAN;
    }

    // check if index is below the start of the table
    {
        let (x, y) = table[0];
        if index < x {
            return y;
        }
    }

    let size = table.len();
    {
        let (x, y) = table[size - 1];
        if index > x {
            return y;
        }
    }
    // binary search seems to be the most appropriate choice here.
    let mut low = 0;
    let mut high = size;
    while low < high {
        let mid = low + (high - low) / 2;
        if table[mid].0 < index {
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    let i = low;
    if approx_eq!(f64, table[i].0, index) {
        table[i].1
    } else {
        // slope = deltaY/deltaX
        let slope = (table[i].1 - table[i - 1].1) / (table[i].0 - table[i - 1].0);
        // y = m*x + b
        (index - table[i - 1].0) * slope + table[i - 1].1
    }
}
