// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Ident};
use crate::datamodel::{Dt, SimMethod, SimSpecs};
use crate::float::SimFloat;

pub(crate) const TIME_OFF: usize = 0;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum Method {
    Euler,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Specs<F: SimFloat> {
    pub start: F,
    pub stop: F,
    pub dt: F,
    pub save_step: F,
    pub method: Method,
}

impl<F: SimFloat> Specs<F> {
    /// Convert this `Specs<F>` to `Specs<F2>` for a different float type.
    pub fn convert<F2: SimFloat>(&self) -> Specs<F2> {
        Specs {
            start: F2::from_f64(self.start.to_f64()),
            stop: F2::from_f64(self.stop.to_f64()),
            dt: F2::from_f64(self.dt.to_f64()),
            save_step: F2::from_f64(self.save_step.to_f64()),
            method: self.method,
        }
    }

    pub fn from(specs: &SimSpecs) -> Self {
        let dt: F = match &specs.dt {
            Dt::Dt(value) => F::from_f64(*value),
            Dt::Reciprocal(value) => F::one() / F::from_f64(*value),
        };

        let save_step: F = match &specs.save_step {
            None => dt,
            Some(save_step) => match save_step {
                Dt::Dt(value) => F::from_f64(*value),
                Dt::Reciprocal(value) => F::one() / F::from_f64(*value),
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

        Specs {
            start: F::from_f64(specs.start),
            stop: F::from_f64(specs.stop),
            dt,
            save_step,
            method,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct Results<F: SimFloat> {
    pub offsets: HashMap<Ident<Canonical>, usize>,
    // one large allocation
    pub data: Box<[F]>,
    pub step_size: usize,
    pub step_count: usize,
    pub specs: Specs<F>,
    pub is_vensim: bool,
}

impl<F: SimFloat> Results<F> {
    pub fn print_tsv(&self) {
        self.print_tsv_comparison(None)
    }
    pub fn print_tsv_comparison(&self, reference: Option<&Results<F>>) {
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
    pub fn iter(&self) -> std::iter::Take<std::slice::Chunks<'_, F>> {
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
        assert_eq!(specs.save_step, 0.5);
    }

    #[test]
    fn specs_from_f32() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: Dt::Dt(0.25),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        let specs: Specs<f32> = Specs::from(&sim_specs);
        assert_eq!(specs.start, 0.0_f32);
        assert_eq!(specs.stop, 100.0_f32);
        assert_eq!(specs.dt, 0.25_f32);
        assert_eq!(specs.save_step, 1.0_f32);
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
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

        let specs: Specs<f64> = Specs::from(&sim_specs);
        assert_eq!(specs.method, Method::Euler);
    }

    #[test]
    fn results_iter_yields_correct_steps() {
        let specs = Specs {
            start: 0.0_f64,
            stop: 2.0,
            dt: 1.0,
            save_step: 1.0,
            method: Method::Euler,
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

    #[test]
    fn results_iter_f32() {
        let specs = Specs {
            start: 0.0_f32,
            stop: 1.0,
            dt: 1.0,
            save_step: 1.0,
            method: Method::Euler,
        };

        let data: Box<[f32]> = vec![0.0f32, 1.0, 2.0, 3.0].into_boxed_slice();
        let results = Results {
            offsets: HashMap::new(),
            data,
            step_size: 2,
            step_count: 2,
            specs,
            is_vensim: false,
        };

        let steps: Vec<&[f32]> = results.iter().collect();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], &[0.0f32, 1.0]);
        assert_eq!(steps[1], &[2.0f32, 3.0]);
    }

    #[test]
    fn specs_convert_f64_to_f32() {
        let specs = Specs {
            start: 0.0_f64,
            stop: 100.0,
            dt: 0.25,
            save_step: 1.0,
            method: Method::Euler,
        };

        let converted: Specs<f32> = specs.convert();
        assert_eq!(converted.start, 0.0_f32);
        assert_eq!(converted.stop, 100.0_f32);
        assert_eq!(converted.dt, 0.25_f32);
        assert_eq!(converted.save_step, 1.0_f32);
        assert_eq!(converted.method, Method::Euler);
    }

    #[test]
    fn specs_convert_f32_to_f64() {
        let specs = Specs {
            start: 0.0_f32,
            stop: 50.0,
            dt: 0.5,
            save_step: 2.0,
            method: Method::Euler,
        };

        let converted: Specs<f64> = specs.convert();
        assert_eq!(converted.start, 0.0_f64);
        assert_eq!(converted.stop, 50.0_f64);
        assert_eq!(converted.dt, 0.5_f64);
        assert_eq!(converted.save_step, 2.0_f64);
    }
}
