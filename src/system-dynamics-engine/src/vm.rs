// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::BorrowMut;
use std::collections::HashMap;

use float_cmp::approx_eq;

use crate::bytecode::{BuiltinId, ByteCode, ByteCodeContext, CompiledModule, ModuleId, Opcode};
use crate::common::{Ident, Result};
use crate::datamodel::{Dimension, Dt, SimMethod, SimSpecs};
use crate::sim_err;

pub(crate) const TIME_OFF: usize = 0;
pub(crate) const DT_OFF: usize = 1;
pub(crate) const INITIAL_TIME_OFF: usize = 2;
pub(crate) const FINAL_TIME_OFF: usize = 3;
pub(crate) const IMPLICIT_VAR_COUNT: usize = 4;

// reserve the last 16 registers as inputs for modules and builtin functions.
// none of our builtins are reentrant, and we copy inputs into the module_args
// slice in the VM, and this avoids having to think about spilling variables.
pub(crate) const FIRST_CALL_REG: u8 = 240u8;

pub(crate) fn is_truthy(n: f64) -> bool {
    let is_false = approx_eq!(f64, n, 0.0);
    !is_false
}

#[derive(Debug)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<Ident, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: String,
    pub(crate) offsets: HashMap<Ident, usize>,
}

#[derive(Clone, Debug)]
struct CompiledSlicedSimulation<'sim> {
    initial_modules: HashMap<&'sim str, CompiledModuleSlice<'sim>>,
    flow_modules: HashMap<&'sim str, CompiledModuleSlice<'sim>>,
    stock_modules: HashMap<&'sim str, CompiledModuleSlice<'sim>>,
    specs: Specs,
    root: &'sim str,
    offsets: &'sim HashMap<Ident, usize>,
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
}

impl Results {
    pub fn print_tsv(&self) {
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

        // print header
        for (i, id) in var_names.iter().enumerate() {
            print!("{}", id);
            if i == var_names.len() - 1 {
                println!();
            } else {
                print!("\t");
            }
        }

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

    pub fn iter(&self) -> std::iter::Take<std::slice::Chunks<f64>> {
        self.data.chunks(self.step_size).take(self.step_count)
    }
}

#[derive(Clone, Debug)]
pub struct VM<'sim> {
    compiled_sim: &'sim CompiledSimulation,
    sliced_sim: CompiledSlicedSimulation<'sim>,
    n_slots: usize,
}

#[derive(Default, Debug)]
struct RegisterCache {
    #[allow(clippy::vec_box)]
    cached_register_files: Vec<Box<[f64; 256]>>,
}

impl RegisterCache {
    #[inline(always)]
    fn get_register_file(&mut self) -> Box<[f64; 256]> {
        self.cached_register_files
            .pop()
            .unwrap_or_else(|| self.new_register_file())
    }

    #[inline(always)]
    fn put_register_file(&mut self, reg: Box<[f64; 256]>) {
        self.cached_register_files.push(reg);
    }

    #[inline(never)]
    fn new_register_file(&self) -> Box<[f64; 256]> {
        Box::new([0.0; 256])
    }
}

#[derive(Clone, Debug)]
struct CompiledModuleSlice<'sim> {
    ident: &'sim str,
    context: &'sim ByteCodeContext,
    bytecode: &'sim ByteCode,
    part: StepPart,
}

impl<'sim> CompiledModuleSlice<'sim> {
    fn new(module: &'sim CompiledModule, part: StepPart) -> Self {
        CompiledModuleSlice {
            ident: module.ident.as_str(),
            context: &module.context,
            bytecode: match part {
                StepPart::Initials => &module.compiled_initials,
                StepPart::Flows => &module.compiled_flows,
                StepPart::Stocks => &module.compiled_stocks,
            },
            part,
        }
    }
}

impl<'sim> VM<'sim> {
    pub fn new(sim: &'sim CompiledSimulation) -> Result<VM> {
        let module = &sim.modules[&sim.root];
        let n_slots = module.n_slots;

        Ok(VM {
            compiled_sim: sim,
            sliced_sim: CompiledSlicedSimulation {
                initial_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.as_str(), CompiledModuleSlice::new(m, StepPart::Initials)))
                    .collect(),
                flow_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.as_str(), CompiledModuleSlice::new(m, StepPart::Flows)))
                    .collect(),
                stock_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| (id.as_str(), CompiledModuleSlice::new(m, StepPart::Stocks)))
                    .collect(),
                specs: sim.specs.clone(),
                root: sim.root.as_str(),
                offsets: &sim.offsets,
            },
            n_slots,
        })
    }

    #[inline(never)]
    pub fn run_to_end(&self) -> Result<Results> {
        let spec = &self.compiled_sim.specs;

        let module_initials = &self.sliced_sim.initial_modules[self.compiled_sim.root.as_str()];
        let module_flows = &self.sliced_sim.flow_modules[self.compiled_sim.root.as_str()];
        let module_stocks = &self.sliced_sim.stock_modules[self.compiled_sim.root.as_str()];

        if spec.stop < spec.start {
            return sim_err!(BadSimSpecs, "".to_string());
        }
        let save_step = if spec.save_step > spec.dt {
            spec.save_step
        } else {
            spec.dt
        };
        let n_chunks: usize = ((spec.stop - spec.start) / save_step + 1.0) as usize;
        let slab: Vec<f64> = vec![0.0; self.n_slots * (n_chunks + 1)];
        let mut data = slab.into_boxed_slice();

        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5).floor() as usize);

        let dt = spec.dt;
        let stop = spec.stop;

        {
            let mut reg_cache = RegisterCache::default();
            let mut slabs = data.chunks_mut(self.n_slots);
            let module_inputs: &[f64; 16] = &[0.0; 16];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = spec.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = spec.start;
            curr[FINAL_TIME_OFF] = spec.stop;
            self.eval(
                module_initials,
                0,
                module_inputs,
                curr,
                next,
                &mut reg_cache,
            );
            let mut is_initial_timestep = true;
            let mut step = 0;
            loop {
                self.eval(module_flows, 0, module_inputs, curr, next, &mut reg_cache);
                self.eval(module_stocks, 0, module_inputs, curr, next, &mut reg_cache);
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
            assert!(curr[TIME_OFF] > stop);
        }

        Ok(Results {
            offsets: self.compiled_sim.offsets.clone(),
            data,
            step_size: self.n_slots,
            step_count: n_chunks,
            specs: spec.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn eval_module(
        &self,
        parent_module: &CompiledModuleSlice,
        parent_module_off: usize,
        module_inputs: &[f64; 16],
        curr: &mut [f64],
        next: &mut [f64],
        reg_cache: &mut RegisterCache,
        id: ModuleId,
    ) {
        let new_module_decl = &parent_module.context.modules[id as usize];
        let model_name = new_module_decl.model_name.as_str();
        let module = match parent_module.part {
            StepPart::Initials => &self.sliced_sim.initial_modules[model_name],
            StepPart::Flows => &self.sliced_sim.flow_modules[model_name],
            StepPart::Stocks => &self.sliced_sim.stock_modules[model_name],
        };

        let module_off = parent_module_off + new_module_decl.off;
        self.eval(module, module_off, &module_inputs, curr, next, reg_cache);
    }

    fn eval(
        &self,
        module: &CompiledModuleSlice,
        module_off: usize,
        module_inputs: &[f64; 16],
        curr: &mut [f64],
        next: &mut [f64],
        reg_cache: &mut RegisterCache,
    ) {
        let bytecode = module.bytecode;

        let mut file = reg_cache.get_register_file();
        let reg = &mut *file;
        let mut condition = false;
        let mut subscript_index: Vec<(u16, u16)> = vec![];
        let mut subscript_index_valid = true;

        let mut i = 0;
        let code = &bytecode.code;
        loop {
            let op = code[i].clone();
            match op {
                Opcode::Mov { dst, src } => reg[dst as usize] = reg[src as usize],
                Opcode::Add { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize] + reg[r as usize]
                }
                Opcode::Sub { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize] - reg[r as usize]
                }
                Opcode::Exp { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize].powf(reg[r as usize])
                }
                Opcode::Mul { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize] * reg[r as usize]
                }
                Opcode::Div { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize] / reg[r as usize]
                }
                Opcode::Mod { dest, l, r } => {
                    reg[dest as usize] = reg[l as usize].rem_euclid(reg[r as usize])
                }
                Opcode::Gt { dest, l, r } => {
                    reg[dest as usize] = (reg[l as usize] > reg[r as usize]) as i8 as f64
                }
                Opcode::Gte { dest, l, r } => {
                    reg[dest as usize] = (reg[l as usize] >= reg[r as usize]) as i8 as f64
                }
                Opcode::Lt { dest, l, r } => {
                    reg[dest as usize] = (reg[l as usize] < reg[r as usize]) as i8 as f64
                }
                Opcode::Lte { dest, l, r } => {
                    reg[dest as usize] = (reg[l as usize] <= reg[r as usize]) as i8 as f64
                }
                Opcode::Eq { dest, l, r } => {
                    reg[dest as usize] = {
                        let l = reg[l as usize];
                        let r = reg[r as usize];
                        approx_eq!(f64, l, r) as i8 as f64
                    }
                }
                Opcode::And { dest, l, r } => {
                    reg[dest as usize] =
                        (is_truthy(reg[l as usize]) && is_truthy(reg[r as usize])) as i8 as f64
                }
                Opcode::Or { dest, l, r } => {
                    reg[dest as usize] =
                        (is_truthy(reg[l as usize]) || is_truthy(reg[r as usize])) as i8 as f64
                }
                Opcode::Not { dest, r } => {
                    reg[dest as usize] = !is_truthy(reg[r as usize]) as i8 as f64
                }
                Opcode::LoadConstant { dest, id } => {
                    reg[dest as usize] = bytecode.literals[id as usize];
                }
                Opcode::LoadGlobalVar { dest, off } => {
                    reg[dest as usize] = curr[off as usize];
                }
                Opcode::LoadVar { dest, off } => {
                    reg[dest as usize] = curr[module_off + off as usize];
                }
                Opcode::PushSubscriptIndex { index, bounds } => {
                    let index = reg[index as usize].floor() as u16;
                    if index == 0 || index > bounds {
                        subscript_index_valid = false;
                    } else {
                        // we convert from 1-based to 0-based here
                        subscript_index.push((index - 1, bounds));
                        subscript_index_valid &= true;
                    };
                }
                Opcode::LoadSubscript { dest, off } => {
                    reg[dest as usize] = if subscript_index_valid {
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
                    subscript_index.clear();
                    subscript_index_valid = true;
                }
                Opcode::SetCond { cond } => {
                    condition = is_truthy(reg[cond as usize]);
                }
                Opcode::If { dest, t, f } => {
                    reg[dest as usize] = if condition {
                        reg[t as usize]
                    } else {
                        reg[f as usize]
                    };
                }
                Opcode::LoadModuleInput { dest, input } => {
                    reg[dest as usize] = module_inputs[input as usize];
                }
                Opcode::EvalModule { id } => {
                    let mut module_inputs = [0.0; 16];
                    module_inputs.copy_from_slice(&reg[FIRST_CALL_REG as usize..]);
                    self.eval_module(
                        module,
                        module_off,
                        &module_inputs,
                        curr,
                        next,
                        reg_cache,
                        id,
                    );
                }
                Opcode::AssignCurr { off, value } => {
                    curr[module_off + off as usize] = reg[value as usize];
                }
                Opcode::AssignNext { off, value } => {
                    next[module_off + off as usize] = reg[value as usize];
                }
                Opcode::Apply { dest, func } => {
                    let time = curr[TIME_OFF];
                    let dt = curr[DT_OFF];
                    let a = reg[FIRST_CALL_REG as usize];
                    let b = reg[(FIRST_CALL_REG + 1) as usize];
                    let c = reg[(FIRST_CALL_REG + 2) as usize];
                    reg[dest as usize] = apply(func, time, dt, a, b, c);
                }
                Opcode::Lookup { dest, gf, value } => {
                    let index = reg[value as usize];
                    let gf = &module.context.graphical_functions[gf as usize];
                    reg[dest as usize] = lookup(gf, index);
                }
                Opcode::Ret => {
                    break;
                }
            }
            i += 1;
        }
        reg_cache.put_register_file(file);
    }

    /*
    pub fn debug_print_bytecode(&self, _model_name: &str) {
        let modules = &self.compiled_sim.modules;
        let mut model_names: Vec<_> = modules.keys().collect();
        model_names.sort_unstable();
        for model_name in model_names {
            eprintln!("\n\nCOMPILED MODEL: {}", model_name);
            let module = &modules[model_name];

            eprintln!("\ninitial literals:");
            for (i, lit) in module.compiled_initials.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\ninital bytecode:");
            for op in module.compiled_initials.code.iter() {
                eprintln!("\t{:?}", op);
            }

            eprintln!("\nflows literals:");
            for (i, lit) in module.compiled_flows.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\nflows bytecode:");
            for op in module.compiled_flows.code.iter() {
                eprintln!("\t{:?}", op);
            }

            eprintln!("\nstocks literals:");
            for (i, lit) in module.compiled_stocks.literals.iter().enumerate() {
                eprintln!("\t{}: {}", i, lit);
            }

            eprintln!("\nstocks bytecode:");
            for op in module.compiled_stocks.code.iter() {
                eprintln!("\t{:?}", op);
            }
        }
    }
     */
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
            size: arrays.iter().map(|v| v.elements.len()).product(),
            lengths: arrays.iter().map(|v| v.elements.len()).collect(),
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
    let empty_dim = Dimension {
        name: "".to_string(),
        elements: vec![],
    };
    let one_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned()],
    };
    let two_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned(), "1".to_owned()],
    };
    let three_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    };
    let cases: &[(Vec<Dimension>, Vec<Vec<usize>>)] = &[
        (vec![empty_dim.clone()], vec![]),
        (vec![empty_dim.clone(), empty_dim.clone()], vec![]),
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
            vec![three_dim.clone(), one_dim.clone(), two_dim.clone()],
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
                .map(|(i, elem)| self.dims[i].elements[*elem].as_str())
                .collect()
        })
    }
}

#[test]
fn test_subscript_iter() {
    let empty_dim = Dimension {
        name: "".to_string(),
        elements: vec![],
    };
    let one_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned()],
    };
    let two_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned(), "1".to_owned()],
    };
    let three_dim = Dimension {
        name: "".to_string(),
        elements: vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    };
    let cases: &[(Vec<Dimension>, Vec<Vec<&str>>)] = &[
        (vec![empty_dim.clone()], vec![]),
        (vec![empty_dim.clone(), empty_dim.clone()], vec![]),
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
            vec![three_dim.clone(), one_dim.clone(), two_dim.clone()],
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
