// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::{Ast, BinaryOp};
use crate::bytecode::CompiledModule;
use crate::compiler::{BuiltinFn, Expr, Module, UnaryOp};
use crate::model::enumerate_modules;
use crate::sim_err;
use crate::vm::{
    CompiledSimulation, DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, Specs,
    StepPart, SubscriptIterator, TIME_OFF, is_truthy, pulse, ramp, step,
};
use crate::{Ident, Project, Results, Variable, compiler, quoteize};
use float_cmp::approx_eq;
use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::rc::Rc;

pub struct ModuleEvaluator<'a> {
    step_part: StepPart,
    off: usize,
    inputs: &'a [f64],
    curr: &'a mut [f64],
    next: &'a mut [f64],
    module: &'a Module,
    sim: &'a Simulation,
}

impl ModuleEvaluator<'_> {
    fn eval(&mut self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n, _) => *n,
            Expr::Dt(_) => self.curr[DT_OFF],
            Expr::ModuleInput(off, _) => self.inputs[*off],
            Expr::EvalModule(ident, model_name, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let module_offsets = &self.module.offsets[&self.module.ident];
                let off = self.off + module_offsets[ident].0;
                let module = &self.sim.modules[model_name.as_str()];

                self.sim
                    .calc(self.step_part, module, off, &args, self.curr, self.next);

                0.0
            }
            Expr::Var(off, _) => self.curr[self.off + *off],
            Expr::Subscript(off, r, bounds, _) => {
                let indices: Vec<_> = r.iter().map(|r| self.eval(r)).collect();
                let mut index = 0;
                let max_bounds = bounds.iter().product();
                let mut ok = true;
                assert_eq!(indices.len(), bounds.len());
                for (i, rhs) in indices.into_iter().enumerate() {
                    let bounds = bounds[i];
                    let one_index = rhs.floor() as usize;
                    if one_index == 0 || one_index > bounds {
                        ok = false;
                        break;
                    } else {
                        index *= bounds;
                        index += one_index - 1;
                    }
                }
                if !ok || index > max_bounds {
                    // 3.7.1 Arrays: If a subscript expression results in an invalid subscript index (i.e., it is out of range), a zero (0) MUST be returned[10]
                    // note 10: Note this can be NaN if so specified in the <uses_arrays> tag of the header options block
                    // 0 makes less sense than NaN, so lets do that until real models force us to do otherwise
                    f64::NAN
                } else {
                    self.curr[self.off + *off + index]
                }
            }
            Expr::AssignCurr(off, r) => {
                let rhs = self.eval(r);
                if self.off + *off > self.curr.len() {
                    unreachable!();
                }
                self.curr[self.off + *off] = rhs;
                0.0
            }
            Expr::AssignNext(off, r) => {
                let rhs = self.eval(r);
                self.next[self.off + *off] = rhs;
                0.0
            }
            Expr::If(cond, t, f, _) => {
                let cond: f64 = self.eval(cond);
                if is_truthy(cond) {
                    self.eval(t)
                } else {
                    self.eval(f)
                }
            }
            Expr::Op1(op, l, _) => {
                let l = self.eval(l);
                match op {
                    UnaryOp::Not => (!is_truthy(l)) as i8 as f64,
                }
            }
            Expr::Op2(op, l, r, _) => {
                let l = self.eval(l);
                let r = self.eval(r);
                match op {
                    BinaryOp::Add => l + r,
                    BinaryOp::Sub => l - r,
                    BinaryOp::Exp => l.powf(r),
                    BinaryOp::Mul => l * r,
                    BinaryOp::Div => l / r,
                    BinaryOp::Mod => l.rem_euclid(r),
                    BinaryOp::Gt => (l > r) as i8 as f64,
                    BinaryOp::Gte => (l >= r) as i8 as f64,
                    BinaryOp::Lt => (l < r) as i8 as f64,
                    BinaryOp::Lte => (l <= r) as i8 as f64,
                    BinaryOp::Eq => approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::Neq => !approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::And => (is_truthy(l) && is_truthy(r)) as i8 as f64,
                    BinaryOp::Or => (is_truthy(l) || is_truthy(r)) as i8 as f64,
                }
            }
            Expr::App(builtin, _) => {
                match builtin {
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => {
                        let off = match builtin {
                            BuiltinFn::Time => TIME_OFF,
                            BuiltinFn::TimeStep => DT_OFF,
                            BuiltinFn::StartTime => INITIAL_TIME_OFF,
                            BuiltinFn::FinalTime => FINAL_TIME_OFF,
                            _ => unreachable!(),
                        };
                        self.curr[off]
                    }
                    BuiltinFn::Abs(a) => self.eval(a).abs(),
                    BuiltinFn::Cos(a) => self.eval(a).cos(),
                    BuiltinFn::Sin(a) => self.eval(a).sin(),
                    BuiltinFn::Tan(a) => self.eval(a).tan(),
                    BuiltinFn::Arccos(a) => self.eval(a).acos(),
                    BuiltinFn::Arcsin(a) => self.eval(a).asin(),
                    BuiltinFn::Arctan(a) => self.eval(a).atan(),
                    BuiltinFn::Exp(a) => self.eval(a).exp(),
                    BuiltinFn::Inf => f64::INFINITY,
                    BuiltinFn::Pi => std::f64::consts::PI,
                    BuiltinFn::Int(a) => self.eval(a).floor(),
                    BuiltinFn::IsModuleInput(ident, _) => {
                        self.module.inputs.contains(ident) as i8 as f64
                    }
                    BuiltinFn::Ln(a) => self.eval(a).ln(),
                    BuiltinFn::Log10(a) => self.eval(a).log10(),
                    BuiltinFn::SafeDiv(a, b, default) => {
                        let a = self.eval(a);
                        let b = self.eval(b);

                        if b != 0.0 {
                            a / b
                        } else if let Some(c) = default {
                            self.eval(c)
                        } else {
                            0.0
                        }
                    }
                    BuiltinFn::Sqrt(a) => self.eval(a).sqrt(),
                    BuiltinFn::Min(a, b) => {
                        let a = self.eval(a);
                        if let Some(b) = b {
                            let b = self.eval(b);
                            // we can't use std::cmp::min here, becuase f64 is only
                            // PartialOrd
                            if a < b { a } else { b }
                        } else {
                            unreachable!();
                        }
                    }
                    BuiltinFn::Mean(args) => {
                        let count = args.len() as f64;
                        let sum: f64 = args.iter().map(|arg| self.eval(arg)).sum();
                        sum / count
                    }
                    BuiltinFn::Max(a, b) => {
                        let a = self.eval(a);
                        if let Some(b) = b {
                            let b = self.eval(b);
                            // we can't use std::cmp::min here, becuase f64 is only
                            // PartialOrd
                            if a > b { a } else { b }
                        } else {
                            unreachable!();
                        }
                    }
                    BuiltinFn::Lookup(id, index, _) => {
                        if !self.module.tables.contains_key(id) {
                            eprintln!("bad lookup for {id}");
                            unreachable!();
                        }
                        let table = &self.module.tables[id].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
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
                            let slope =
                                (table[i].1 - table[i - 1].1) / (table[i].0 - table[i - 1].0);
                            // y = m*x + b
                            (index - table[i - 1].0) * slope + table[i - 1].1
                        }
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let dt = self.curr[DT_OFF];
                        let volume = self.eval(a);
                        let first_pulse = self.eval(b);
                        let interval = match c.as_ref() {
                            Some(c) => self.eval(c),
                            None => 0.0,
                        };

                        pulse(time, dt, volume, first_pulse, interval)
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let slope = self.eval(a);
                        let start_time = self.eval(b);
                        let end_time = c.as_ref().map(|c| self.eval(c));

                        ramp(time, slope, start_time, end_time)
                    }
                    BuiltinFn::Step(a, b) => {
                        let time = self.curr[TIME_OFF];
                        let dt = self.curr[DT_OFF];
                        let height = self.eval(a);
                        let step_time = self.eval(b);

                        step(time, dt, height, step_time)
                    }
                    BuiltinFn::Rank(_, _)
                    | BuiltinFn::Size(_)
                    | BuiltinFn::Stddev(_)
                    | BuiltinFn::Sum(_) => {
                        unreachable!();
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Simulation {
    pub(crate) modules: HashMap<Ident, Module>,
    specs: Specs,
    root: String,
    offsets: HashMap<Ident, usize>,
}

impl Simulation {
    pub fn new(project: &Project, main_model_name: &str) -> crate::Result<Self> {
        if !project.models.contains_key(main_model_name) {
            return sim_err!(
                NotSimulatable,
                format!("no model named '{}' to simulate", main_model_name)
            );
        }

        let modules = {
            let project_models: HashMap<_, _> = project
                .models
                .iter()
                .map(|(name, model)| (name.as_str(), model.as_ref()))
                .collect();
            // then pull in all the module instantiations the main model depends on
            enumerate_modules(&project_models, main_model_name, |model| model.name.clone())?
        };

        let module_names: Vec<&str> = {
            let mut module_names: Vec<&str> = modules.keys().map(|id| id.as_str()).collect();
            module_names.sort_unstable();

            let mut sorted_names = vec![main_model_name];
            sorted_names.extend(module_names.into_iter().filter(|n| *n != main_model_name));
            sorted_names
        };

        let mut compiled_modules: HashMap<Ident, Module> = HashMap::new();
        for name in module_names {
            let distinct_inputs = &modules[name];
            for inputs in distinct_inputs.iter() {
                let model = Rc::clone(&project.models[name]);
                let is_root = name == main_model_name;
                let module = Module::new(project, model, inputs, is_root)?;
                compiled_modules.insert(name.to_string(), module);
            }
        }

        let specs = Specs::from(&project.datamodel.sim_specs);

        let offsets = calc_flattened_offsets(project, main_model_name);
        let offsets: HashMap<Ident, usize> =
            offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: main_model_name.to_string(),
            offsets,
        })
    }

    pub fn compile(&self) -> crate::Result<CompiledSimulation> {
        let modules: crate::Result<HashMap<String, CompiledModule>> = self
            .modules
            .iter()
            .map(|(name, module)| module.compile().map(|module| (name.clone(), module)))
            .collect();

        Ok(CompiledSimulation {
            modules: modules?,
            specs: self.specs.clone(),
            root: self.root.clone(),
            offsets: self.offsets.clone(),
        })
    }

    pub fn runlist_order(&self) -> Vec<Ident> {
        calc_flattened_order(self, "main")
    }

    pub fn debug_print_runlists(&self, _model_name: &str) {
        let mut model_names: Vec<_> = self.modules.keys().collect();
        model_names.sort_unstable();
        for model_name in model_names {
            eprintln!("\n\nMODEL: {model_name}");
            let module = &self.modules[model_name];
            let offsets = &module.offsets[model_name];
            let mut idents: Vec<_> = offsets.keys().collect();
            idents.sort_unstable();

            eprintln!("offsets");
            for ident in idents {
                let (off, size) = offsets[ident];
                eprintln!("\t{ident}: {off}, {size}");
            }

            eprintln!("\ninital runlist:");
            for expr in module.runlist_initials.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }

            eprintln!("\nflows runlist:");
            for expr in module.runlist_flows.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }

            eprintln!("\nstocks runlist:");
            for expr in module.runlist_stocks.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }
        }
    }

    fn calc(
        &self,
        step_part: StepPart,
        module: &Module,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let runlist = match step_part {
            StepPart::Initials => &module.runlist_initials,
            StepPart::Flows => &module.runlist_flows,
            StepPart::Stocks => &module.runlist_stocks,
        };

        let mut step = ModuleEvaluator {
            step_part,
            off: module_off,
            curr,
            next,
            module,
            inputs: module_inputs,
            sim: self,
        };

        for expr in runlist.iter() {
            step.eval(expr);
        }
    }

    fn n_slots(&self, module_name: &str) -> usize {
        self.modules[module_name].n_slots
    }

    pub fn run_to_end(&self) -> crate::Result<Results> {
        let spec = &self.specs;
        if spec.stop < spec.start {
            return sim_err!(BadSimSpecs, "".to_string());
        }
        let save_step = if spec.save_step > spec.dt {
            spec.save_step
        } else {
            spec.dt
        };
        let n_chunks: usize = ((spec.stop - spec.start) / save_step + 1.0) as usize;
        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5).floor() as usize);

        let dt = spec.dt;
        let stop = spec.stop;

        let n_slots = self.n_slots(&self.root);

        let module = &self.modules[&self.root];

        let slab: Vec<f64> = vec![0.0; n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);

            let module_inputs: &[f64] = &[];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = self.specs.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = self.specs.start;
            curr[FINAL_TIME_OFF] = self.specs.stop;
            self.calc(StepPart::Initials, module, 0, module_inputs, curr, next);
            let mut is_initial_timestep = true;
            let mut step = 0;
            loop {
                self.calc(StepPart::Flows, module, 0, module_inputs, curr, next);
                self.calc(StepPart::Stocks, module, 0, module_inputs, curr, next);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                next[DT_OFF] = dt;
                curr[INITIAL_TIME_OFF] = self.specs.start;
                curr[FINAL_TIME_OFF] = self.specs.stop;
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
            offsets: self.offsets.clone(),
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
            specs: spec.clone(),
            is_vensim: false,
        })
    }
}

/// calc_flattened_offsets generates a mapping from name to offset
/// for all individual variables and subscripts in a model, including
/// in submodels.  For example a variable named "offset" in a module
/// instantiated with name "sector" will produce the key "sector.offset".
pub fn calc_flattened_offsets(
    project: &Project,
    model_name: &str,
) -> HashMap<Ident, (usize, usize)> {
    let is_root = model_name == "main";

    let mut offsets: HashMap<Ident, (usize, usize)> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert("time".to_string(), (0, 1));
        offsets.insert("dt".to_string(), (1, 1));
        offsets.insert("initial_time".to_string(), (2, 1));
        offsets.insert("final_time".to_string(), (3, 1));
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    for ident in var_names.iter() {
        let size = if let Variable::Module { model_name, .. } = &model.variables[*ident] {
            let sub_offsets = calc_flattened_offsets(project, model_name);
            let mut sub_var_names: Vec<&str> = sub_offsets.keys().map(|v| v.as_str()).collect();
            sub_var_names.sort_unstable();
            for sub_name in sub_var_names {
                let (sub_off, sub_size) = sub_offsets[sub_name];
                offsets.insert(
                    format!("{}.{}", quoteize(ident), quoteize(sub_name)),
                    (i + sub_off, sub_size),
                );
            }
            let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
            sub_size
        } else if let Some(Ast::ApplyToAll(dims, _)) = &model.variables[*ident].ast() {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let subscripted_ident = format!("{}[{}]", quoteize(ident), subscript);
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = &model.variables[*ident].ast() {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let subscripted_ident = format!("{}[{}]", quoteize(ident), subscript);
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else {
            offsets.insert(quoteize(ident), (i, 1));
            1
        };
        i += size;
    }

    offsets
}

fn calc_flattened_order(sim: &Simulation, model_name: &str) -> Vec<Ident> {
    let is_root = model_name == "main";

    let module = &sim.modules[model_name];

    let mut offsets: Vec<Ident> = Vec::with_capacity(module.runlist_order.len() + 1);

    if is_root {
        offsets.push("time".to_owned());
    }

    for ident in module.runlist_order.iter() {
        // FIXME: this isnt' quite right (assumes no regular var has same name as module)
        if sim.modules.contains_key(ident) {
            let sub_var_names = calc_flattened_order(sim, ident);
            for sub_name in sub_var_names.iter() {
                offsets.push(format!("{}.{}", quoteize(ident), quoteize(sub_name)));
            }
        } else {
            offsets.push(quoteize(ident));
        }
    }

    offsets
}

#[test]
fn test_arrays() {
    use crate::ast::Loc;
    use crate::compiler::{Context, Var};
    use std::collections::BTreeSet;

    let project = {
        use crate::datamodel::*;
        Project {
            name: "arrays".to_owned(),
            source: None,
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 12.0,
                dt: Dt::Dt(0.25),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_owned()),
            },
            dimensions: vec![Dimension::Named(
                "letters".to_owned(),
                vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            )],
            units: vec![],
            models: vec![Model {
                name: "main".to_owned(),
                variables: vec![
                    Variable::Aux(Aux {
                        ident: "constants".to_owned(),
                        equation: Equation::Arrayed(
                            vec!["letters".to_owned()],
                            vec![
                                ("a".to_owned(), "9".to_owned(), None),
                                ("b".to_owned(), "7".to_owned(), None),
                                ("c".to_owned(), "5".to_owned(), None),
                            ],
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked".to_owned(),
                        equation: Equation::Scalar("aux[INT(TIME MOD 5) + 1]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "aux".to_owned(),
                        equation: Equation::ApplyToAll(
                            vec!["letters".to_owned()],
                            "constants".to_owned(),
                            None,
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked2".to_owned(),
                        equation: Equation::Scalar("aux[b]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                ],
                views: vec![],
            }],
        }
    };

    let parsed_project = Rc::new(Project::from(project));

    {
        let actual = calc_flattened_offsets(&parsed_project, "main");
        let expected: HashMap<_, _> = vec![
            ("time".to_owned(), (0, 1)),
            ("dt".to_owned(), (1, 1)),
            ("initial_time".to_owned(), (2, 1)),
            ("final_time".to_owned(), (3, 1)),
            ("aux[a]".to_owned(), (4, 1)),
            ("aux[b]".to_owned(), (5, 1)),
            ("aux[c]".to_owned(), (6, 1)),
            ("constants[a]".to_owned(), (7, 1)),
            ("constants[b]".to_owned(), (8, 1)),
            ("constants[c]".to_owned(), (9, 1)),
            ("picked".to_owned(), (10, 1)),
            ("picked2".to_owned(), (11, 1)),
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
    }

    let metadata = compiler::build_metadata(&parsed_project, "main", true);
    let main_metadata = &metadata["main"];
    assert_eq!(main_metadata["aux"].offset, 4);
    assert_eq!(main_metadata["aux"].size, 3);
    assert_eq!(main_metadata["constants"].offset, 7);
    assert_eq!(main_metadata["constants"].size, 3);
    assert_eq!(main_metadata["picked"].offset, 10);
    assert_eq!(main_metadata["picked"].size, 1);
    assert_eq!(main_metadata["picked2"].offset, 11);
    assert_eq!(main_metadata["picked2"].size, 1);

    let module_models = compiler::calc_module_model_map(&parsed_project, "main");

    let arrayed_constants_var = &parsed_project.models["main"].variables["constants"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: arrayed_constants_var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
        },
        arrayed_constants_var,
    );

    assert!(parsed_var.is_ok());

    let expected = Var {
        ident: arrayed_constants_var.ident().to_owned(),
        ast: vec![
            Expr::AssignCurr(7, Box::new(Expr::Const(9.0, Loc::default()))),
            Expr::AssignCurr(8, Box::new(Expr::Const(7.0, Loc::default()))),
            Expr::AssignCurr(9, Box::new(Expr::Const(5.0, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let arrayed_aux_var = &parsed_project.models["main"].variables["aux"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: arrayed_aux_var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
        },
        arrayed_aux_var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: arrayed_aux_var.ident().to_owned(),
        ast: vec![
            Expr::AssignCurr(4, Box::new(Expr::Var(7, Loc::default()))),
            Expr::AssignCurr(5, Box::new(Expr::Var(8, Loc::default()))),
            Expr::AssignCurr(6, Box::new(Expr::Var(9, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let var = &parsed_project.models["main"].variables["picked2"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: var.ident().to_owned(),
        ast: vec![Expr::AssignCurr(
            11,
            Box::new(Expr::Subscript(
                4,
                vec![Expr::Const(2.0, Loc::default())],
                vec![3],
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let var = &parsed_project.models["main"].variables["picked"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: var.ident().to_owned(),
        ast: vec![Expr::AssignCurr(
            10,
            Box::new(Expr::Subscript(
                4,
                vec![Expr::Op2(
                    BinaryOp::Add,
                    Box::new(Expr::App(
                        BuiltinFn::Int(Box::new(Expr::Op2(
                            BinaryOp::Mod,
                            Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
                            Box::new(Expr::Const(5.0, Loc::default())),
                            Loc::default(),
                        ))),
                        Loc::default(),
                    )),
                    Box::new(Expr::Const(1.0, Loc::default())),
                    Loc::default(),
                )],
                vec![3],
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let sim = Simulation::new(&parsed_project, "main");
    assert!(sim.is_ok());
}

#[test]
fn nan_is_approx_eq() {
    assert!(approx_eq!(f64, f64::NAN, f64::NAN));
}
