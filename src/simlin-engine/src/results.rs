// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::Write;

use crate::common::{Canonical, Ident};
use crate::datamodel::{Dt, SimMethod, SimSpecs};

pub(crate) const TIME_OFF: usize = 0;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum Method {
    Euler,
    RungeKutta2,
    RungeKutta4,
}

/// Its `f64` time specs are compared with the DERIVED (IEEE) `PartialEq`; see
/// the "Float equality in this crate" section on `crate::ast::Literal` for the
/// project's position on float equality in cache keys (GH #642). A NaN here is
/// not reachable from a valid `SimSpecs`, so only the signed-zero direction
/// applies, and it is inert for a start/stop/dt.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
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
            SimMethod::RungeKutta2 => Method::RungeKutta2,
            SimMethod::RungeKutta4 => Method::RungeKutta4,
        };

        // Truncation (not round) is correct: for non-divisible save_step
        // values only save points within [start, stop] are counted.
        //
        // The effective save cadence is max(save_step, dt) because the VM
        // cannot save more often than once per dt step
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
        let stdout = std::io::stdout();
        self.write_tsv(&mut stdout.lock(), reference)
            .expect("writing the results to stdout");
    }

    /// The saved columns: every key of the offsets map with its slot, in slot
    /// order (ties by name, so the order is a function of the map).
    ///
    /// A slot the map has no key for -- a standalone lookup table, a helper
    /// slot the map hides -- is not a column. The map is the contract every
    /// reader of a series shares, and an unnamed slot holds a backend's
    /// scratch value, which printed beside the named ones would read as a
    /// series.
    fn columns(&self) -> Vec<(&Ident<Canonical>, usize)> {
        let mut columns: Vec<(&Ident<Canonical>, usize)> = self
            .offsets
            .iter()
            .map(|(name, off)| (name, *off))
            .collect();
        columns.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        columns
    }

    /// Write the results as TSV: a header of the column names, then one row
    /// per saved step up to the run's stop time. With a `reference`, the
    /// header starts with a `series` column and every step is two rows,
    /// `reference` (blank where the reference has no such column) and
    /// `simlin`.
    pub fn write_tsv<W: Write>(
        &self,
        out: &mut W,
        reference: Option<&Results>,
    ) -> std::io::Result<()> {
        let columns = self.columns();
        fn row<W: Write>(out: &mut W, cells: impl Iterator<Item = String>) -> std::io::Result<()> {
            for (i, cell) in cells.enumerate() {
                if i > 0 {
                    write!(out, "\t")?;
                }
                write!(out, "{cell}")?;
            }
            writeln!(out)
        }

        let names = columns.iter().map(|(name, _)| name.to_string());
        match reference {
            Some(_) => row(out, std::iter::once("series".to_string()).chain(names))?,
            None => row(out, names)?,
        }
        let mut reference_rows = reference.map(|reference| reference.iter());
        for curr in self.iter() {
            if curr[TIME_OFF] > self.specs.stop {
                break;
            }
            let values = columns.iter().map(|(_, off)| curr[*off].to_string());
            let Some(reference) = reference else {
                row(out, values)?;
                continue;
            };
            let Some(ref_curr) = reference_rows.as_mut().and_then(Iterator::next) else {
                break;
            };
            let reference_values = columns.iter().map(|(name, _)| {
                reference
                    .offsets
                    .get(*name)
                    .map_or_else(String::new, |off| ref_curr[*off].to_string())
            });
            row(
                out,
                std::iter::once("reference".to_string()).chain(reference_values),
            )?;
            row(out, std::iter::once("simlin".to_string()).chain(values))?;
        }
        Ok(())
    }
    pub fn iter(&self) -> std::iter::Take<std::slice::Chunks<'_, f64>> {
        self.data.chunks(self.step_size).take(self.step_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone lookup table keeps a layout slot but has no key in the
    /// offsets map (GH #606), so a run over one has more slots than names.
    /// The TSV is the map's columns and nothing else: the header lists every
    /// key in slot order and no other column, every row has that many cells,
    /// and the comparison form prefixes a `series` column and pairs each
    /// step's `reference` and `simlin` rows.
    #[test]
    fn tsv_prints_the_offset_maps_columns_and_no_unnamed_slot() {
        use crate::test_common::TestProject;

        let table = crate::datamodel::GraphicalFunction {
            kind: crate::datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0, 2.0]),
            y_points: vec![0.0, 5.0, 10.0],
            x_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
            y_scale: crate::datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 10.0,
            },
        };
        let tp = TestProject::new("tsv_columns")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux_with_gf("table", "", table)
            .aux("y", "LOOKUP(table, TIME)", None);
        let compiled = tp.compile_incremental().expect("compiles");
        let mut vm = crate::vm::Vm::new(compiled).expect("vm");
        vm.run_to_end().expect("runs");
        let results = vm.into_results();
        assert!(
            results.step_size > results.offsets.len(),
            "the table's slot must be unnamed, or this run exercises nothing"
        );
        let mut expected: Vec<(String, usize)> = results
            .offsets
            .iter()
            .map(|(name, off)| (name.to_string(), *off))
            .collect();
        expected.sort_by_key(|(_, off)| *off);
        let expected_header: Vec<String> = expected.into_iter().map(|(name, _)| name).collect();
        assert!(expected_header.contains(&"y".to_string()));

        let mut out = Vec::new();
        results.write_tsv(&mut out, None).expect("writes");
        let text = String::from_utf8(out).expect("utf-8");
        let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
        assert_eq!(
            rows[0], expected_header,
            "the header is the map's keys in slot order"
        );
        assert_eq!(rows.len(), 1 + results.step_count, "one row per saved step");
        for row in &rows[1..] {
            assert_eq!(
                row.len(),
                expected_header.len(),
                "every row has one cell per column"
            );
        }
        let y = expected_header
            .iter()
            .position(|n| n == "y")
            .expect("y column");
        assert_eq!(rows[2][y], "5", "y = LOOKUP(table, 1) at step 1");

        let mut out = Vec::new();
        results.write_tsv(&mut out, Some(&results)).expect("writes");
        let text = String::from_utf8(out).expect("utf-8");
        let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
        let mut header = vec!["series".to_string()];
        header.extend(expected_header.iter().cloned());
        assert_eq!(rows[0], header);
        assert_eq!(rows.len(), 1 + 2 * results.step_count);
        for pair in rows[1..].chunks(2) {
            assert_eq!(pair[0][0], "reference");
            assert_eq!(pair[1][0], "simlin");
            assert_eq!(
                pair[0][1..],
                pair[1][1..],
                "a run compared with itself agrees"
            );
        }
    }

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
    fn specs_from_rk2() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta2,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.method, Method::RungeKutta2);
    }

    #[test]
    fn specs_from_rk4() {
        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta4,
            time_units: None,
        };

        let specs = Specs::from(&sim_specs);
        assert_eq!(specs.method, Method::RungeKutta4);
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
