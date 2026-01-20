// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Ident};
use crate::datamodel::{Dt, SimMethod, SimSpecs};

pub(crate) const TIME_OFF: usize = 0;

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
    pub offsets: HashMap<Ident<Canonical>, usize>,
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
        let unknown = Ident::<Canonical>::from_unchecked("UNKNOWN".to_string());
        let var_names = {
            let offset_name_map: HashMap<usize, &Ident<Canonical>> =
                self.offsets.iter().map(|(k, v)| (*v, k)).collect();
            let mut var_names: Vec<&Ident<Canonical>> = Vec::with_capacity(self.step_size);
            for i in 0..(self.step_size) {
                let name = if offset_name_map.contains_key(&i) {
                    offset_name_map[&i]
                } else {
                    &unknown
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
            print!("{id}");
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
                            print!("{val}");
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
                        print!("{val}");
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
                        print!("{val}");
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
    pub fn iter(&self) -> std::iter::Take<std::slice::Chunks<'_, f64>> {
        self.data.chunks(self.step_size).take(self.step_count)
    }
}
