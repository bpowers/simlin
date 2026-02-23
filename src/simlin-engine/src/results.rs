// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Ident};
use crate::datamodel::{Dt, SimMethod, SimSpecs};

pub(crate) const TIME_OFF: usize = 0;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum Method {
    Euler,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Specs {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    pub save_step: f64,
    pub method: Method,
    /// Number of saved output timesteps, pre-computed from the original f64
    /// spec values.  Using truncation (floor) so non-divisible save_step
    /// values don't over-allocate beyond the simulation horizon.
    pub n_chunks: usize,
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
            SimMethod::RungeKutta2 => {
                eprintln!("warning, simulation requested 'rk2', but only support Euler");
                Method::Euler
            }
            SimMethod::RungeKutta4 => {
                eprintln!("warning, simulation requested 'rk4', but only support Euler");
                Method::Euler
            }
        };

        // Truncation (not round) is correct: for non-divisible save_step
        // values only save points within [start, stop] are counted.
        //
        // The effective save cadence is max(save_step, dt) because the VM
        // and interpreter cannot save more often than once per dt step
        // (save_every = max(1, round(save_step/dt))).
        let effective_save_step = if save_step > dt { save_step } else { dt };
        let n_chunks = ((specs.stop - specs.start) / effective_save_step + 1.0) as usize;

        Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            save_step,
            method,
            n_chunks,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_from_dt_value() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: Dt::Dt(0.25),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.start, 0.0);
        assert_eq!(specs.stop, 100.0);
        assert_eq!(specs.dt, 0.25);
        assert_eq!(specs.save_step, 0.25); // defaults to dt when save_step is None
        assert_eq!(specs.method, Method::Euler);
    }

    #[test]
    fn specs_from_dt_reciprocal() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Reciprocal(4.0), // 1/4 = 0.25
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.dt, 0.25);
    }

    #[test]
    fn specs_from_with_save_step() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: Dt::Dt(0.25),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.dt, 0.25);
        assert_eq!(specs.save_step, 1.0);
    }

    #[test]
    fn specs_from_with_reciprocal_save_step() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: Dt::Dt(0.25),
            save_step: Some(Dt::Reciprocal(2.0)), // 1/2 = 0.5
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.save_step, 0.5);
    }

    #[test]
    fn specs_from_rk2_warns() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta2,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        // Falls back to Euler with a warning
        assert_eq!(specs.method, Method::Euler);
    }

    #[test]
    fn specs_from_rk4_warns() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta4,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.method, Method::Euler);
    }

    #[test]
    fn results_iter_yields_correct_steps() {
        let specs = Specs {
            start: 0.0,
            stop: 2.0,
            dt: 1.0,
            save_step: 1.0,
            method: Method::Euler,
            n_chunks: 3,
        };

        // 2 variables, 3 steps (0, 1, 2)
        let data: Box<[f64]> = vec![
            0.0, 10.0, // step 0
            1.0, 20.0, // step 1
            2.0, 30.0, // step 2
        ]
        .into_boxed_slice();

        let results = Results {
            offsets: HashMap::new(),
            data,
            step_size: 2,
            step_count: 3,
            specs,
            is_vensim: false,
        };

        let steps: Vec<&[f64]> = results.iter().collect();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], &[0.0, 10.0]);
        assert_eq!(steps[1], &[1.0, 20.0]);
        assert_eq!(steps[2], &[2.0, 30.0]);
    }

    // ── n_chunks tests ────────────────────────────────────────────────

    #[test]
    fn specs_n_chunks_divisible() {
        // start=0, stop=10, save_step=1 → 11 save points (0,1,...,10)
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.n_chunks, 11);
    }

    #[test]
    fn specs_n_chunks_non_divisible() {
        // start=0, stop=10, save_step=4 → 3 save points (0,4,8); 12 > stop
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(4.0)),
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.n_chunks, 3);
    }

    #[test]
    fn specs_n_chunks_non_divisible_three() {
        // start=0, stop=10, save_step=3 → 4 save points (0,3,6,9); 12 > stop
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(3.0)),
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.n_chunks, 4);
    }

    #[test]
    fn specs_n_chunks_save_step_smaller_than_dt() {
        // save_step=0.5 < dt=1.0: can't save more often than once per dt,
        // so effective save cadence is dt=1.0, giving 11 steps for [0,10].
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(0.5)),
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.n_chunks, 11);
    }
}
