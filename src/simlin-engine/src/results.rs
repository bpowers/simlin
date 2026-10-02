// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::Write;

use crate::common::{Canonical, Ident};
use crate::datamodel::{Dt, SimMethod, SimSpecs};

/// The slot of a results row that holds its time: the VM's own.
pub(crate) const TIME_OFF: usize = crate::vm::TIME_OFF;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum Method {
    Euler,
    RungeKutta2,
    RungeKutta4,
}

/// How close to a whole number of steps a time has to be to count as that
/// step, in steps: a millionth. A time a person writes as a step's (`0.3` at a
/// DT of `0.1`, or `1900.3` from a start of `1900`) is a few units in the last
/// place off the step it names, and so is the quotient that finds its index;
/// the tolerance is what reads it as that step rather than the one before.
/// It is small enough that a time a person means as between two steps
/// (`3.00001` at a DT of 1) is between them.
///
/// It is a fraction of a step, not of the times: it absorbs the rounding of
/// the quotient, and the rounding of the times themselves only while that is
/// under a millionth of DT. Where a time's own last place is coarser than
/// that (a start of `1e9` with a DT of `1e-3`, where an f64 holds times to
/// about `1e-7`), a time typed as a step's can read as the step before.
pub(crate) const STEP_TOLERANCE: f64 = 1e-6;

/// The specs a run is made under, on the step grid.
///
/// A run's clock is counted, never accumulated: step `k` is at
/// [`Specs::time_at`]`(k)`, `start + k * dt`, so the time of a step is the
/// nearest number to the time it names, however long the run is. The run
/// evaluates the steps `0..=`[`Specs::final_step`].
///
/// It saves `n_chunks` rows, one for each save time `start + m * save_step`
/// up to the stop time: row `m` is the first step at or after its save time
/// ([`Specs::saved_row_step`]). With a save step that is a whole number of DTs
/// that step is at the save time; otherwise it is up to one DT after it, and
/// the rows are not evenly spaced. This is the engine's rule: it keeps the
/// rows a declared save step asks for (as many, each at the time asked for or
/// the step after, the stop time among them when it is a save time), where
/// rounding the save step to a whole number of DTs would drift from the
/// declared times as the run goes on. XMILE 1.0 has no save step
/// (`isee:save_interval` is a vendor attribute), and Vensim's documentation of
/// SAVEPER says only that TIME STEP should be equal to or smaller than it, so
/// what either does with one off the DT grid is unverified.
///
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
    /// The time between two save times: the declared save step, or DT when
    /// none is declared or the declared one is shorter than a step (a run
    /// saves a step at most once).
    pub save_step: f64,
    pub method: Method,
    /// The rows a whole run saves.
    pub n_chunks: usize,
}

/// Whether a save step is a whole number of DTs, to within the step
/// tolerance: every saved row is then at its save time.
pub fn save_step_is_on_the_step_grid(dt: f64, save_step: f64) -> bool {
    let steps = save_step / dt;
    (steps - steps.round()).abs() <= STEP_TOLERANCE
}

impl Specs {
    pub fn from(specs: &SimSpecs) -> Self {
        let dt: f64 = match &specs.dt {
            Dt::Dt(value) => *value,
            Dt::Reciprocal(value) => 1.0 / *value,
        };

        let declared_save_step: f64 = match &specs.save_step {
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

        let mut specs = Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            // Written so that a save step that is not a number is DT too.
            save_step: if declared_save_step > dt {
                declared_save_step
            } else {
                dt
            },
            method,
            n_chunks: 0,
        };
        specs.n_chunks = specs.saved_rows_through(specs.final_step());
        specs
    }

    /// Why no run can be made under these specs, if none can: a stop time
    /// before the start time, or a DT that is not a positive number. The one
    /// statement both backends refuse specs by (`Vm::with_specs`,
    /// `wasmgen::compile_simulation`). Any positive DT is accepted, however
    /// small: whether its run can be held is the backend's question.
    pub fn refusal(&self) -> Option<&'static str> {
        if self.stop < self.start {
            Some("end time has to be after start time")
        } else if self.dt <= 0.0 || self.dt.is_nan() {
            Some("dt must be greater than 0")
        } else {
            None
        }
    }

    /// The index of the last step of the run: the last step at or before the
    /// stop time (zero for specs that describe no run, which
    /// [`Specs::refusal`] refuses).
    pub fn final_step(&self) -> u64 {
        // A saturating cast: NaN and a negative count are zero steps.
        self.step_at_or_before(self.stop) as u64
    }

    /// The time of step `step`.
    pub fn time_at(&self, step: u64) -> f64 {
        self.start + step as f64 * self.dt
    }

    /// The index of the last step at or before `time`, as a number of steps:
    /// it is compared with a step count, which an `f64` holds exactly, and a
    /// time before the start is before step zero (negative). The wasm backend
    /// computes the same expression (`wasmgen::module::emit_run_to`), so the
    /// two stop at the same step for any target.
    pub fn step_at_or_before(&self, time: f64) -> f64 {
        ((time - self.start) / self.dt + STEP_TOLERANCE).floor()
    }

    /// The index of the first step at or after `time`, as a number of steps.
    pub fn step_at_or_after(&self, time: f64) -> f64 {
        ((time - self.start) / self.dt - STEP_TOLERANCE).ceil()
    }

    /// The save step in DTs: a whole number when it is within the step
    /// tolerance of one, so a save step on the step grid saves exactly every
    /// so many steps; at least one; and always a finite number, whatever the
    /// specs, so that row zero is step zero ([`Specs::saved_row_step`]
    /// multiplies a row by it, and zero times an infinite ratio is no step).
    pub fn save_step_in_steps(&self) -> f64 {
        let steps = self.save_step / self.dt;
        let steps = if save_step_is_on_the_step_grid(self.dt, self.save_step) {
            steps.round()
        } else {
            steps
        };
        // A ratio that is not a number saves every step (`clamp` would pass
        // it through), and a save step whose ratio to DT overflows is one no
        // run reaches a second row of.
        if steps.is_nan() {
            1.0
        } else {
            steps.clamp(1.0, f64::MAX)
        }
    }

    /// The step saved row `row` is: the first step at or after the row's save
    /// time, `row` save steps from the start; as a number of steps, for
    /// comparing with a step count. Both backends decide whether a step is
    /// saved by comparing its index with this for the next row
    /// (`Vm::run_steps`, `wasmgen::module::emit_save_advance`), in the same
    /// f64 operations, so they save the same steps.
    pub fn saved_row_step(&self, row: u64) -> f64 {
        Self::saved_row_step_at(self.save_step_in_steps(), row)
    }

    /// [`Specs::saved_row_step`] for a save step of `save_step_in_steps` DTs.
    pub fn saved_row_step_at(save_step_in_steps: f64, row: u64) -> f64 {
        (row as f64 * save_step_in_steps - STEP_TOLERANCE).ceil()
    }

    /// How many rows a run that has evaluated the steps `0..=step` has saved.
    fn saved_rows_through(&self, step: u64) -> usize {
        let every = self.save_step_in_steps();
        let saved = |row: u64| Self::saved_row_step_at(every, row) <= step as f64;
        // The quotient is the answer to within a row; the two loops settle it
        // by the rule the run itself saves by.
        let mut last_row = (step as f64 / every) as u64;
        while saved(last_row.saturating_add(1)) && last_row < u64::MAX {
            last_row += 1;
        }
        while last_row > 0 && !saved(last_row) {
            last_row -= 1;
        }
        usize::try_from(last_row)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
    }
}

/// `x` as a person writes it: in decimal where that is short (a magnitude
/// from a ten-thousandth to a quadrillion, or zero), else with an exponent
/// (`1e-320`, `6.51e63`), in the fewest digits that read back as `x`. A
/// plain `{}` writes every digit of the expansion: 320 of them for a DT of
/// `1e-320`.
pub(crate) fn written(x: f64) -> String {
    if x == 0.0 || !x.is_finite() || (1e-4..1e15).contains(&x.abs()) {
        format!("{x}")
    } else {
        format!("{x:e}")
    }
}

/// A count as a person writes it: every digit up to a quadrillion, and past
/// it three significant ones (`4e18`), since a float holds no more of so
/// large a count than that and its last digits would be noise
/// (`4.0000000000000005e18`).
pub(crate) fn written_count(count: f64) -> String {
    if count.abs() < 1e15 {
        written(count.round())
    } else {
        written(format!("{count:.2e}").parse().unwrap_or(count))
    }
}

/// A count of rows as a person writes it, the count a run's rows saturate
/// at (`usize::MAX`, `Specs::n_chunks` for a run whose rows cannot be
/// counted) named as what it is.
pub(crate) fn written_rows(rows: usize) -> String {
    if rows == usize::MAX {
        "more rows than can be counted".to_string()
    } else {
        format!("{} rows", written_count(rows as f64))
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Results {
    pub offsets: HashMap<Ident<Canonical>, usize>,
    /// The rows, step-major, in one allocation. It may be longer than the
    /// rows saved (a VM's buffer holds its working rows after them); `iter`
    /// is the saved rows.
    pub data: Box<[f64]>,
    pub step_size: usize,
    /// The rows saved: every one of them a step the run evaluated, in time
    /// order. A whole run saves `specs.n_chunks`; a run stopped part-way
    /// saves fewer.
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
    /// per saved step. With a `reference`, the
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
        assert_eq!(
            specs.save_step, 1.0,
            "a step is saved at most once, so the save step is DT"
        );
    }

    fn grid(start: f64, stop: f64, dt: f64, save_step: Option<f64>) -> Specs {
        Specs::from(&SimSpecs {
            start,
            stop,
            dt: Dt::Dt(dt),
            save_step: save_step.map(Dt::Dt),
            sim_method: SimMethod::Euler,
            time_units: None,
        })
    }

    /// The last step is the one a person's stop time names, though the
    /// quotient that finds it is a few units in the last place off a whole
    /// number: on either side of it, from zero and from a start in the
    /// thousands. A stop time between two steps ends the run at the one
    /// before it.
    #[test]
    fn the_final_step_is_the_step_the_stop_time_names() {
        for (start, stop, dt, last) in [
            (0.0, 0.3, 0.1, 3),
            (0.0, 0.7, 0.1, 7),
            (0.0, 1.4, 0.2, 7),
            (0.0, 0.35, 0.05, 7),
            (1.0, 2.3, 0.1, 13),
            (1900.0, 1900.3, 0.1, 3),
            (2000.0, 2000.07, 0.01, 7),
            (0.0, 100.0, 0.1, 1000),
            (0.0, 10.0, 0.3, 33),
            (0.0, 10.0, 3.0, 3),
            (0.0, 10.0, 1.0, 10),
            (5.0, 5.0, 1.0, 0),
        ] {
            let specs = grid(start, stop, dt, None);
            assert_eq!(specs.final_step(), last, "{start}..{stop} by {dt}");
            assert_eq!(specs.n_chunks as u64, last + 1, "{start}..{stop} by {dt}");
            // The quotients these rows exist for are not whole numbers.
            assert!(
                specs.time_at(last) <= stop + dt * STEP_TOLERANCE,
                "{start}..{stop} by {dt}"
            );
        }
    }

    /// A step's time is `start + k * dt`, the nearest number to the time it
    /// names: at a DT of 0.1 the fiftieth step is at 5, which fifty additions
    /// of 0.1 are not.
    #[test]
    fn a_steps_time_is_counted_not_accumulated() {
        let specs = grid(0.0, 100.0, 0.1, None);
        assert_eq!(specs.time_at(50), 5.0);
        assert_eq!(specs.time_at(1000), 100.0);
        let accumulated = (0..50).fold(0.0, |t, _| t + 0.1);
        assert_ne!(accumulated, 5.0, "or the row above shows nothing");
    }

    /// The step at or before a time, and at or after it: a time between two
    /// steps takes the one before or the one after, and a time written as a
    /// step's is that step both ways, whichever side of a whole number its
    /// quotient falls on (`2.1 / 0.3` is above 7, `0.3 / 0.1` below 3).
    #[test]
    fn a_time_written_as_a_steps_is_that_step() {
        let specs = grid(0.0, 100.0, 0.1, None);
        for (time, before, after) in [
            (0.3, 3.0, 3.0),
            (1.1, 11.0, 11.0),
            (5.0, 50.0, 50.0),
            (0.34, 3.0, 4.0),
            (0.36, 3.0, 4.0),
            (0.0, 0.0, 0.0),
            (-0.05, -1.0, 0.0),
        ] {
            assert_eq!(specs.step_at_or_before(time), before, "before {time}");
            assert_eq!(specs.step_at_or_after(time), after, "after {time}");
        }
        // A time a hundred-thousandth of a step off a step is between two
        // steps: the tolerance is for rounding, not for times near a step.
        let whole = grid(0.0, 100.0, 1.0, None);
        for (time, before, after) in [
            (3.00001, 3.0, 4.0),
            (2.99999, 2.0, 3.0),
            (3.000_000_01, 3.0, 3.0),
            (2.999_999_99, 3.0, 3.0),
        ] {
            assert_eq!(whole.step_at_or_before(time), before, "before {time}");
            assert_eq!(whole.step_at_or_after(time), after, "after {time}");
        }
        assert!(!save_step_is_on_the_step_grid(1.0, 2.00001));
        assert!(save_step_is_on_the_step_grid(1.0, 2.000_000_01));
        assert_eq!(grid(0.0, 10.0, 1.0, Some(2.00001)).saved_row_step(1), 3.0);
        let thirds = grid(0.0, 10.0, 0.3, None);
        assert_eq!(thirds.step_at_or_before(2.1), 7.0);
        assert_eq!(thirds.step_at_or_after(2.1), 7.0);
        assert!(2.1 / thirds.dt > 7.0 && 0.3 / specs.dt < 3.0);
        let quarters = grid(0.0, 8.0, 0.25, None);
        assert_eq!(quarters.step_at_or_after(5.1), 21.0);
        assert_eq!(quarters.step_at_or_before(5.1), 20.0);
        let from_one = grid(1.0, 9.0, 0.5, None);
        assert_eq!(from_one.step_at_or_after(0.0), -2.0, "before the start");
    }

    /// A save step that is a whole number of DTs saves every so many steps. One
    /// that is not saves, for each save time, the first step at or after it:
    /// as many rows as the save step asks for, the stop time's among them
    /// when it is a save time, each within a DT of its save time.
    #[test]
    fn a_row_is_the_first_step_at_or_after_its_save_time() {
        let rows = |specs: &Specs| -> Vec<f64> {
            (0..specs.n_chunks as u64)
                .map(|row| specs.time_at(specs.saved_row_step(row) as u64))
                .collect()
        };
        let on_grid = grid(0.0, 10.0, 0.25, Some(1.0));
        assert_eq!(rows(&on_grid), (0..=10).map(f64::from).collect::<Vec<_>>());
        assert_eq!(on_grid.save_step, 1.0);

        let off_grid = grid(0.0, 10.0, 1.0, Some(2.5));
        assert_eq!(rows(&off_grid), [0.0, 3.0, 5.0, 8.0, 10.0]);
        assert_eq!(off_grid.save_step, 2.5, "the declared step is the specs'");

        // A DT of 1/128 with a decimal save step, the specs of the corpus's
        // `Single_Pendulum.mdl`: 1001 rows over 100 time units, none drifting.
        let fine = grid(0.0, 100.0, 0.0078125, Some(0.1));
        let times = rows(&fine);
        assert_eq!(times.len(), 1001);
        assert_eq!(times[1000], 100.0);
        for (row, time) in times.iter().enumerate() {
            let save_time = row as f64 * 0.1;
            assert!(
                *time >= save_time - 1e-9 && *time < save_time + 0.0078125,
                "row {row} at {time}"
            );
        }
        assert!(times.windows(2).all(|pair| pair[1] > pair[0]));

        // A save time that is a step's is that step, though the product that
        // finds it is a unit in the last place over a whole number
        // (`3 * (0.13 / 0.03)` is `13.000000000000002`).
        let thirds = grid(0.0, 10.0, 0.03, Some(0.13));
        assert!(3.0 * thirds.save_step_in_steps() > 13.0);
        assert_eq!(thirds.saved_row_step(3), 13.0);
        assert_eq!(thirds.saved_row_step(1), 5.0, "0.13 is between two steps");

        // On the grid to within rounding: 0.3 is three DTs of 0.1.
        let rounded = grid(0.0, 3.0, 0.1, Some(0.3));
        assert_eq!(rounded.save_step_in_steps(), 3.0);
        assert_eq!(rounded.n_chunks, 11);
        assert!(save_step_is_on_the_step_grid(0.1, 0.3));
        assert!(!save_step_is_on_the_step_grid(1.0, 2.5));
    }

    /// Specs that describe no run are zero steps and one row, never a panic
    /// or an overflow: `Vm::new` is what refuses them.
    #[test]
    fn specs_that_describe_no_run_count_no_steps() {
        for (start, stop, dt) in [
            (0.0, 10.0, 0.0),
            (0.0, 10.0, -1.0),
            (0.0, 10.0, f64::NAN),
            (10.0, 0.0, 1.0),
        ] {
            let specs = grid(start, stop, dt, None);
            assert!(specs.n_chunks >= 1, "{start}..{stop} by {dt}");
        }
        // A save step with no ratio to DT (zero over zero) saves every step.
        assert_eq!(grid(0.0, 10.0, 0.0, None).save_step_in_steps(), 1.0);
        // A DT too small to count the steps of saturates.
        assert_eq!(grid(0.0, 1.0, 1e-320, None).n_chunks, usize::MAX);
    }

    /// A save step whose ratio to DT is more than a float holds is a run of
    /// one row, the first step: row zero is step zero whatever the save step.
    #[test]
    fn a_save_step_longer_than_any_run_saves_the_first_step() {
        for save_step in [1e308, f64::MAX] {
            let specs = grid(0.0, 10.0, 0.25, Some(save_step));
            assert!((save_step / 0.25_f64).is_infinite());
            assert_eq!(specs.save_step_in_steps(), f64::MAX);
            assert_eq!(specs.saved_row_step(0), 0.0);
            assert!(specs.saved_row_step(1) > specs.final_step() as f64);
            assert_eq!(specs.n_chunks, 1);
        }
    }
}
