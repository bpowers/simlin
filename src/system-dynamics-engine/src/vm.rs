// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::BorrowMut;
use std::collections::HashMap;

use crate::bytecode::CompiledModule;
use crate::common::{Ident, Result};
use crate::datamodel::{Dt, SimMethod, SimSpecs};
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

#[derive(Debug)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<Ident, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: String,
    pub(crate) offsets: HashMap<Ident, usize>,
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
    n_slots: usize,
}

impl<'sim> VM<'sim> {
    pub fn new(sim: &'sim CompiledSimulation) -> Result<VM> {
        let module = &sim.modules[&sim.root];
        let n_slots = module.n_slots;

        Ok(VM {
            compiled_sim: sim,
            n_slots,
        })
    }

    pub fn run_to_end(&self) -> Result<Results> {
        let spec = &self.compiled_sim.specs;
        let module = &self.compiled_sim.modules[&self.compiled_sim.root];

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
            let mut slabs = data.chunks_mut(self.n_slots);
            let module_inputs: &[f64; 16] = &[0.0; 16];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = spec.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = spec.start;
            curr[FINAL_TIME_OFF] = spec.stop;
            self.eval(StepPart::Initials, module, 0, module_inputs, curr, next);
            let mut is_initial_timestep = true;
            let mut step = 0;
            loop {
                self.eval(StepPart::Flows, module, 0, module_inputs, curr, next);
                self.eval(StepPart::Stocks, module, 0, module_inputs, curr, next);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                next[DT_OFF] = dt;
                curr[INITIAL_TIME_OFF] = spec.start;
                curr[FINAL_TIME_OFF] = spec.stop;
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

    fn eval(
        &self,
        step_part: StepPart,
        module: &CompiledModule,
        module_off: usize,
        module_inputs: &[f64; 16],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let _bytecode = match step_part {
            StepPart::Initials => &module.compiled_initials,
            StepPart::Flows => &module.compiled_flows,
            StepPart::Stocks => &module.compiled_stocks,
        };

        let mut registers: [f64; 256] = [0.0; 256];
        let mut cond = false;
        let mut subscript_index: usize = 0;
    }
}
