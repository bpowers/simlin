// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use float_cmp::approx_eq;
use smallvec::SmallVec;

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledModule, DimId, LookupMode, ModuleId, Op2, Opcode,
    RuntimeView, TempId,
};
use crate::common::{Canonical, Ident, Result};
use crate::datamodel::{Dt, SimMethod, SimSpecs};
use crate::dimensions::{Dimension, match_dimensions_two_pass};
use crate::sim_err;

/// Key for looking up compiled modules.
/// A model can have multiple instantiations with different input sets,
/// and each needs its own compiled module because the ModuleInput offsets differ.
pub type ModuleKey = (Ident<Canonical>, BTreeSet<Ident<Canonical>>);

/// Helper to create a module key from model name and input set
pub fn make_module_key(
    model_name: &Ident<Canonical>,
    input_set: &BTreeSet<Ident<Canonical>>,
) -> ModuleKey {
    (model_name.clone(), input_set.clone())
}

// ============================================================================
// Iteration State (for array iteration during VM execution)
// ============================================================================

/// State for array iteration within the VM.
#[derive(Clone, Debug)]
struct IterState {
    /// Index into view_stack for the source view
    view_stack_idx: usize,
    /// Target temp array ID (if writing to temp)
    write_temp_id: Option<TempId>,
    /// Current flat index in the iteration
    current: usize,
    /// Total number of elements to iterate
    size: usize,
    /// Pre-computed flat offsets for sparse iteration (None if contiguous)
    flat_offsets: Option<Vec<usize>>,
}

/// Info about how one source maps to the broadcast result dimensions.
#[derive(Clone, Debug)]
struct BroadcastSourceInfo {
    /// Index into view_stack for this source
    view_stack_idx: usize,
    /// For each result dimension, which source dimension it maps to.
    /// -1 means this source doesn't have this dimension (broadcast).
    dim_map: SmallVec<[i8; 4]>,
}

/// State for broadcast iteration over multiple sources.
#[derive(Clone, Debug)]
struct BroadcastState {
    /// Info for each source
    sources: SmallVec<[BroadcastSourceInfo; 2]>,
    /// Destination temp array ID
    dest_temp_id: TempId,
    /// Result dimensions (sizes)
    result_dims: SmallVec<[u16; 4]>,
    /// Current multi-dimensional indices in result
    result_indices: SmallVec<[u16; 4]>,
    /// Current flat index in result
    current: usize,
    /// Total result size
    size: usize,
}

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
    pub(crate) modules: HashMap<ModuleKey, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: ModuleKey,
    pub(crate) offsets: HashMap<Ident<Canonical>, usize>,
}

#[derive(Clone, Debug)]
struct CompiledSlicedSimulation {
    initial_modules: HashMap<ModuleKey, CompiledModuleSlice>,
    flow_modules: HashMap<ModuleKey, CompiledModuleSlice>,
    stock_modules: HashMap<ModuleKey, CompiledModuleSlice>,
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

// helper to borrow two non-overlapping chunk slices by index
fn borrow_two(buf: &mut [f64], n_slots: usize, a: usize, b: usize) -> (&mut [f64], &mut [f64]) {
    let (lo, hi, flip) = if a < b { (a, b, false) } else { (b, a, true) };
    let split = hi * n_slots;
    let (left, right) = buf.split_at_mut(split);
    let left = &mut left[lo * n_slots..(lo + 1) * n_slots];
    let right = &mut right[..n_slots];
    if !flip { (left, right) } else { (right, left) }
}

#[derive(Clone, Debug)]
pub struct Vm {
    specs: Specs,
    root: ModuleKey,
    offsets: HashMap<Ident<Canonical>, usize>,
    sliced_sim: CompiledSlicedSimulation,
    n_slots: usize,
    n_chunks: usize,
    // simulation buffer for saved samples and working state
    data: Option<Box<[f64]>>,
    // indices into chunks for current and next slots
    curr_chunk: usize,
    next_chunk: usize,
    // have we completed initials and emitted the first state
    did_initials: bool,
    // step counter for save_every cadence
    step_accum: usize,
    // Temp array storage (allocated once, reused across evals)
    // Indexed by temp_offsets from ByteCodeContext
    temp_storage: Vec<f64>,
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
    ident: Ident<Canonical>,
    context: Arc<ByteCodeContext>,
    bytecode: Arc<ByteCode>,
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
        let root_module = &sim.modules[&sim.root];
        let n_slots = root_module.n_slots;
        let n_chunks: usize = ((sim.specs.stop - sim.specs.start) / save_step + 1.0) as usize;
        let data: Box<[f64]> = vec![0.0; n_slots * (n_chunks + 2)].into_boxed_slice();

        // Allocate temp storage based on context temp info
        let temp_total_size = root_module.context.temp_total_size;
        let temp_storage = vec![0.0; temp_total_size];

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
            curr_chunk: 0,
            next_chunk: 1,
            did_initials: false,
            step_accum: 0,
            temp_storage,
        })
    }
    pub fn run_to_end(&mut self) -> Result<()> {
        let end = self.specs.stop;
        self.run_to(end)
    }

    #[inline(never)]
    pub fn run_to(&mut self, end: f64) -> Result<()> {
        // Copy spec values to avoid holding borrows across eval calls
        let spec_start = self.specs.start;
        let spec_stop = self.specs.stop;
        let dt = self.specs.dt;
        let save_step = self.specs.save_step;
        let n_slots = self.n_slots;
        let n_chunks = self.n_chunks;

        let save_every = std::cmp::max(1, (save_step / dt + 0.5).floor() as usize);

        let mut stack = Stack::new();
        let module_inputs: &[f64] = &[0.0; 0];
        let mut data = None;
        std::mem::swap(&mut data, &mut self.data);
        let mut data = data.unwrap();

        // Initialize initials once
        if !self.did_initials {
            let (curr, next) = borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
            curr[TIME_OFF] = spec_start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = spec_start;
            curr[FINAL_TIME_OFF] = spec_stop;

            let module_initials = &self.sliced_sim.initial_modules[&self.root];
            Self::eval(
                &self.sliced_sim,
                &mut self.temp_storage,
                module_initials,
                0,
                module_inputs,
                curr,
                next,
                &mut stack,
            );
            self.did_initials = true;
            self.step_accum = 0;
        }

        loop {
            let (curr, next) = borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
            if curr[TIME_OFF] > end {
                break;
            }

            let module_flows = &self.sliced_sim.flow_modules[&self.root];
            let module_stocks = &self.sliced_sim.stock_modules[&self.root];

            Self::eval(
                &self.sliced_sim,
                &mut self.temp_storage,
                module_flows,
                0,
                module_inputs,
                curr,
                next,
                &mut stack,
            );
            Self::eval(
                &self.sliced_sim,
                &mut self.temp_storage,
                module_stocks,
                0,
                module_inputs,
                curr,
                next,
                &mut stack,
            );
            next[TIME_OFF] = curr[TIME_OFF] + dt;
            next[DT_OFF] = curr[DT_OFF];
            next[INITIAL_TIME_OFF] = curr[INITIAL_TIME_OFF];
            next[FINAL_TIME_OFF] = curr[FINAL_TIME_OFF];

            self.step_accum += 1;
            let is_initial_timestep = (self.curr_chunk == 0) && (curr[TIME_OFF] == spec_start);
            if self.step_accum != save_every && !is_initial_timestep {
                // copy next into curr
                let (curr2, next2) =
                    borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
                curr2.copy_from_slice(next2);
            } else {
                self.curr_chunk = self.next_chunk;
                if self.next_chunk + 1 >= n_chunks + 2 {
                    break;
                }
                self.next_chunk += 1;
                self.step_accum = 0;
            }
        }
        self.data = Some(data);
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

    pub fn set_value_now(&mut self, off: usize, val: f64) {
        let start = self.curr_chunk * self.n_slots;
        let mut data = None;
        std::mem::swap(&mut data, &mut self.data);
        let mut data = data.unwrap();
        data[start + off] = val;
        self.data = Some(data);
    }

    pub fn get_value_now(&self, off: usize) -> f64 {
        let start = self.curr_chunk * self.n_slots;
        self.data.as_ref().unwrap()[start + off]
    }

    pub fn names_as_strs(&self) -> Vec<String> {
        self.offsets
            .keys()
            .map(|k| k.as_str().to_string())
            .collect()
    }

    pub fn get_offset(&self, ident: &Ident<Canonical>) -> Option<usize> {
        self.offsets.get(ident).copied()
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn eval_module(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        parent_module: &CompiledModuleSlice,
        parent_module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        id: ModuleId,
    ) {
        let new_module_decl = &parent_module.context.modules[id as usize];
        let module_key = make_module_key(&new_module_decl.model_name, &new_module_decl.input_set);
        let module_off = parent_module_off + new_module_decl.off;

        let module = match parent_module.part {
            StepPart::Initials => &sliced_sim.initial_modules[&module_key],
            StepPart::Flows => &sliced_sim.flow_modules[&module_key],
            StepPart::Stocks => &sliced_sim.stock_modules[&module_key],
        };

        Self::eval(
            sliced_sim,
            temp_storage,
            module,
            module_off,
            module_inputs,
            curr,
            next,
            stack,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn eval(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        module: &CompiledModuleSlice,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
    ) {
        let bytecode = &module.bytecode;
        let context = &module.context;

        // Existing state
        let mut condition = false;
        let mut subscript_index: Vec<(u16, u16)> = vec![];
        let mut subscript_index_valid = true;

        // Array support: view stack, iteration stack, broadcast stack (local to this eval)
        let mut view_stack: Vec<RuntimeView> = Vec::with_capacity(4);
        let mut iter_stack: Vec<IterState> = Vec::with_capacity(2);
        let mut broadcast_stack: Vec<BroadcastState> = Vec::with_capacity(1);

        let code = &bytecode.code;

        // PC-based loop for jump support
        let mut pc: usize = 0;
        while pc < code.len() {
            match &code[pc] {
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
                    stack.push(bytecode.literals[*id as usize]);
                }
                Opcode::LoadGlobalVar { off } => {
                    stack.push(curr[*off as usize]);
                }
                Opcode::LoadVar { off } => {
                    stack.push(curr[module_off + *off as usize]);
                }
                Opcode::PushSubscriptIndex { bounds } => {
                    let index = stack.pop().floor() as u16;
                    if index == 0 || index > *bounds {
                        subscript_index_valid = false;
                    } else {
                        // we convert from 1-based to 0-based here
                        subscript_index.push((index - 1, *bounds));
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
                        curr[module_off + *off as usize + index]
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
                    stack.push(module_inputs[*input as usize]);
                }
                Opcode::EvalModule { id, n_inputs } => {
                    use std::iter;
                    let mut module_inputs: SmallVec<[f64; 16]> =
                        iter::repeat_n(0.0, *n_inputs as usize).collect();
                    for j in (0..(*n_inputs as usize)).rev() {
                        module_inputs[j] = stack.pop();
                    }
                    Self::eval_module(
                        sliced_sim,
                        temp_storage,
                        module,
                        module_off,
                        &module_inputs,
                        curr,
                        next,
                        stack,
                        *id,
                    );
                }
                Opcode::AssignCurr { off } => {
                    curr[module_off + *off as usize] = stack.pop();
                    assert_eq!(0, stack.stack.len());
                }
                Opcode::AssignNext { off } => {
                    next[module_off + *off as usize] = stack.pop();
                    assert_eq!(0, stack.stack.len());
                }
                Opcode::Apply { func } => {
                    let time = curr[TIME_OFF];
                    let dt = curr[DT_OFF];
                    let c = stack.pop();
                    let b = stack.pop();
                    let a = stack.pop();

                    stack.push(apply(*func, time, dt, a, b, c));
                }
                Opcode::Lookup {
                    base_gf,
                    table_count,
                    mode,
                } => {
                    let lookup_index = stack.pop();
                    let element_offset = stack.pop();

                    // Bounds check: element_offset must be in [0, table_count)
                    if element_offset < 0.0 || element_offset >= (*table_count as f64) {
                        stack.push(f64::NAN);
                    } else {
                        let gf_idx = (*base_gf as usize) + (element_offset as usize);
                        let gf = &context.graphical_functions[gf_idx];
                        let result = match mode {
                            LookupMode::Interpolate => lookup(gf, lookup_index),
                            LookupMode::Forward => lookup_forward(gf, lookup_index),
                            LookupMode::Backward => lookup_backward(gf, lookup_index),
                        };
                        stack.push(result);
                    }
                }
                Opcode::Ret => {
                    break;
                }

                // =========================================================
                // VIEW STACK OPERATIONS
                // =========================================================
                Opcode::PushVarView {
                    base_off,
                    n_dims,
                    dim_ids,
                } => {
                    // Build a view for a variable with given dimensions
                    let n = *n_dims as usize;
                    let dims: SmallVec<[u16; 4]> = (0..n)
                        .map(|i| context.dimensions[dim_ids[i] as usize].size)
                        .collect();
                    let dim_id_vec: SmallVec<[DimId; 4]> = dim_ids[..n].iter().copied().collect();
                    let view = RuntimeView::for_var(
                        (module_off + *base_off as usize) as u32,
                        dims,
                        dim_id_vec,
                    );
                    view_stack.push(view);
                }

                Opcode::PushTempView {
                    temp_id,
                    n_dims,
                    dim_ids,
                } => {
                    let n = *n_dims as usize;
                    let dims: SmallVec<[u16; 4]> = (0..n)
                        .map(|i| context.dimensions[dim_ids[i] as usize].size)
                        .collect();
                    let dim_id_vec: SmallVec<[DimId; 4]> = dim_ids[..n].iter().copied().collect();
                    let view = RuntimeView::for_temp(*temp_id, dims, dim_id_vec);
                    view_stack.push(view);
                }

                Opcode::PushStaticView { view_id } => {
                    let static_view = &context.static_views[*view_id as usize];
                    view_stack.push(static_view.to_runtime_view());
                }

                Opcode::PushVarViewDirect {
                    base_off,
                    n_dims,
                    dims,
                } => {
                    // Build a view with explicit dimension sizes (no dim_id lookup needed)
                    let n = *n_dims as usize;
                    let dims_vec: SmallVec<[u16; 4]> = dims[..n].iter().copied().collect();
                    // Use 0 as dim_id since we don't have dimension metadata
                    let dim_ids: SmallVec<[DimId; 4]> = (0..n).map(|_| 0 as DimId).collect();
                    let view = RuntimeView::for_var(
                        (module_off + *base_off as usize) as u32,
                        dims_vec,
                        dim_ids,
                    );
                    view_stack.push(view);
                }

                Opcode::ViewSubscriptConst { dim_idx, index } => {
                    let view = view_stack.last_mut().unwrap();
                    view.apply_single_subscript(*dim_idx as usize, *index);
                }

                Opcode::ViewSubscriptDynamic { dim_idx } => {
                    // XMILE uses 1-based indexing; validate bounds and convert to 0-based
                    let index_1based = stack.pop().floor() as u16;
                    let view = view_stack.last_mut().unwrap();
                    // apply_single_subscript_checked validates bounds and sets is_valid=false
                    // if out of bounds, allowing subsequent reads to return NaN
                    view.apply_single_subscript_checked(*dim_idx as usize, index_1based);
                }

                Opcode::ViewRange {
                    dim_idx,
                    start,
                    end,
                } => {
                    let view = view_stack.last_mut().unwrap();
                    view.apply_range(*dim_idx as usize, *start, *end);
                }

                Opcode::ViewRangeDynamic { dim_idx } => {
                    // Pop end and start from stack (1-based indices, inclusive range)
                    let end_1based = stack.pop() as u16;
                    let start_1based = stack.pop() as u16;
                    let view = view_stack.last_mut().unwrap();
                    // apply_range_checked handles validation and 1-based to 0-based conversion
                    view.apply_range_checked(*dim_idx as usize, start_1based, end_1based);
                }

                Opcode::ViewStarRange {
                    dim_idx,
                    subdim_relation_id,
                } => {
                    let rel = &context.subdim_relations[*subdim_relation_id as usize];
                    let view = view_stack.last_mut().unwrap();
                    // Use apply_sparse_with_dim_id to update the dim_id to the child
                    // (subdimension) so broadcasting matches correctly
                    view.apply_sparse_with_dim_id(
                        *dim_idx as usize,
                        rel.parent_offsets.clone(),
                        rel.child_dim_id,
                    );
                }

                Opcode::ViewWildcard { dim_idx: _ } => {
                    // Wildcard is a no-op - dimension stays as-is
                }

                Opcode::ViewTranspose {} => {
                    let view = view_stack.last_mut().unwrap();
                    view.transpose();
                }

                Opcode::PopView {} => {
                    view_stack.pop();
                }

                Opcode::DupView {} => {
                    let top = view_stack.last().unwrap().clone();
                    view_stack.push(top);
                }

                // =========================================================
                // TEMP ARRAY ACCESS
                // =========================================================
                Opcode::LoadTempConst { temp_id, index } => {
                    let temp_off = context.temp_offsets[*temp_id as usize];
                    let value = temp_storage[temp_off + *index as usize];
                    stack.push(value);
                }

                Opcode::LoadTempDynamic { temp_id } => {
                    let index = stack.pop().floor() as usize;
                    let temp_off = context.temp_offsets[*temp_id as usize];
                    let value = temp_storage[temp_off + index];
                    stack.push(value);
                }

                // =========================================================
                // ITERATION
                // =========================================================
                Opcode::BeginIter {
                    write_temp_id,
                    has_write_temp,
                } => {
                    let view = view_stack.last().unwrap();
                    let size = view.size();

                    // Pre-compute flat offsets for iteration
                    let flat_offsets = if view.sparse.is_empty() && view.is_contiguous() {
                        // Contiguous: can iterate directly
                        None
                    } else {
                        // Need to pre-compute all flat offsets
                        let mut offsets = Vec::with_capacity(size);
                        let dims = &view.dims;
                        let n_dims = dims.len();
                        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];

                        for _ in 0..size {
                            offsets.push(view.flat_offset(&indices));

                            // Increment indices (row-major order)
                            for d in (0..n_dims).rev() {
                                indices[d] += 1;
                                if indices[d] < dims[d] {
                                    break;
                                }
                                indices[d] = 0;
                            }
                        }
                        Some(offsets)
                    };

                    iter_stack.push(IterState {
                        view_stack_idx: view_stack.len() - 1,
                        write_temp_id: if *has_write_temp {
                            Some(*write_temp_id)
                        } else {
                            None
                        },
                        current: 0,
                        size,
                        flat_offsets,
                    });
                }

                Opcode::LoadIterElement {} => {
                    let iter_state = iter_stack.last().unwrap();
                    let view = &view_stack[iter_state.view_stack_idx];

                    // Return NaN for invalid views (e.g., out-of-bounds subscript)
                    if !view.is_valid {
                        stack.push(f64::NAN);
                    } else {
                        let flat_off = if let Some(ref offsets) = iter_state.flat_offsets {
                            offsets[iter_state.current]
                        } else {
                            // Contiguous: flat offset = base_off + offset + current
                            view.offset as usize + iter_state.current
                        };

                        let value = if view.is_temp {
                            let temp_off = context.temp_offsets[view.base_off as usize];
                            temp_storage[temp_off + flat_off]
                        } else {
                            curr[view.base_off as usize + flat_off]
                        };
                        stack.push(value);
                    }
                }

                Opcode::LoadIterTempElement { temp_id } => {
                    let iter_state = iter_stack.last().unwrap();
                    let temp_off = context.temp_offsets[*temp_id as usize];
                    let value = temp_storage[temp_off + iter_state.current];
                    stack.push(value);
                }

                Opcode::LoadIterViewTop {} => {
                    // Load from the view on TOP of view_stack (not iter_state's view)
                    // using the current iteration index from iter_state.
                    // This allows loading from multiple different source arrays in one loop.
                    //
                    // Supports broadcasting: if source has fewer dimensions than iteration,
                    // uses dim_ids to match dimensions and broadcasts along missing axes.
                    //
                    // For indexed dimensions of the same size but different dim_ids,
                    // uses positional matching as a fallback.
                    //
                    // Returns NaN for out-of-bounds access (when source is smaller than iteration).
                    let iter_state = iter_stack.last().unwrap();
                    let source_view = view_stack.last().unwrap();

                    if !source_view.is_valid {
                        stack.push(f64::NAN);
                    } else {
                        // Get the iteration view (output dimensions)
                        let iter_view = &view_stack[iter_state.view_stack_idx];

                        // Fast path: if dimensions match exactly, use simple offset calculation
                        let result = if source_view.dims == iter_view.dims
                            && source_view.dim_ids == iter_view.dim_ids
                        {
                            // Bounds check: if source is smaller than iteration, return NaN
                            if iter_state.current >= source_view.size() {
                                None
                            } else {
                                Some(source_view.offset_for_iter_index(iter_state.current))
                            }
                        } else {
                            // Broadcasting path: source has different dimensions
                            // 1. Decompose iteration index into multi-dimensional indices
                            let iter_dims = &iter_view.dims;
                            let mut iter_indices: SmallVec<[u16; 4]> = SmallVec::new();
                            let mut remaining = iter_state.current;

                            for &dim in iter_dims.iter().rev() {
                                iter_indices.push((remaining % dim as usize) as u16);
                                remaining /= dim as usize;
                            }
                            iter_indices.reverse();

                            // 2. Pre-compute which dimensions are indexed
                            let source_is_indexed: SmallVec<[bool; 4]> = source_view
                                .dim_ids
                                .iter()
                                .map(|&dim_id| {
                                    context
                                        .dimensions
                                        .get(dim_id as usize)
                                        .is_some_and(|d| d.is_indexed)
                                })
                                .collect();
                            let iter_is_indexed: SmallVec<[bool; 4]> = iter_view
                                .dim_ids
                                .iter()
                                .map(|&dim_id| {
                                    context
                                        .dimensions
                                        .get(dim_id as usize)
                                        .is_some_and(|d| d.is_indexed)
                                })
                                .collect();

                            // 3. Use shared two-pass dimension matching algorithm
                            let source_to_iter = match_dimensions_two_pass(
                                &source_view.dim_ids,
                                &source_view.dims,
                                &source_is_indexed,
                                &iter_view.dim_ids,
                                &iter_view.dims,
                                &iter_is_indexed,
                            );

                            // 4. Build source indices from mapping
                            let mut source_indices: SmallVec<[u16; 4]> =
                                SmallVec::with_capacity(source_view.dims.len());
                            let mut out_of_bounds = false;

                            for (src_dim_pos, mapped_iter_pos) in source_to_iter.iter().enumerate()
                            {
                                if let Some(iter_pos) = mapped_iter_pos {
                                    let idx = iter_indices[*iter_pos];
                                    // Bounds check for this dimension
                                    if idx >= source_view.dims[src_dim_pos] {
                                        out_of_bounds = true;
                                        break;
                                    }
                                    source_indices.push(idx);
                                } else {
                                    // No matching dimension found - this is a compiler bug
                                    // or dimension mismatch. Return NaN.
                                    out_of_bounds = true;
                                    break;
                                }
                            }

                            if out_of_bounds {
                                None
                            } else {
                                // 5. Compute flat offset using source view
                                Some(source_view.flat_offset(&source_indices))
                            }
                        };

                        if let Some(flat_off) = result {
                            let value = if source_view.is_temp {
                                let temp_off = context.temp_offsets[source_view.base_off as usize];
                                temp_storage[temp_off + flat_off]
                            } else {
                                curr[source_view.base_off as usize + flat_off]
                            };
                            stack.push(value);
                        } else {
                            // Out of bounds or no matching dimension - return NaN
                            stack.push(f64::NAN);
                        }
                    }
                }

                Opcode::StoreIterElement {} => {
                    let value = stack.pop();
                    let iter_state = iter_stack.last().unwrap();

                    if let Some(write_temp_id) = iter_state.write_temp_id {
                        let temp_off = context.temp_offsets[write_temp_id as usize];
                        temp_storage[temp_off + iter_state.current] = value;
                    } else {
                        panic!("StoreIterElement without write_temp");
                    }
                }

                Opcode::NextIterOrJump { jump_back } => {
                    let iter_state = iter_stack.last_mut().unwrap();
                    iter_state.current += 1;

                    if iter_state.current < iter_state.size {
                        // Jump backward to loop start
                        pc = (pc as isize + *jump_back as isize) as usize;
                        continue; // Skip pc increment
                    }
                    // else: iteration done, continue to next opcode
                }

                Opcode::EndIter {} => {
                    iter_stack.pop();
                }

                // =========================================================
                // ARRAY REDUCTIONS
                // =========================================================
                Opcode::ArraySum {} => {
                    let view = view_stack.last().unwrap();
                    let sum =
                        Self::reduce_view(temp_storage, view, curr, context, |acc, v| acc + v, 0.0);
                    stack.push(sum);
                }

                Opcode::ArrayMax {} => {
                    let view = view_stack.last().unwrap();
                    let max = Self::reduce_view(
                        temp_storage,
                        view,
                        curr,
                        context,
                        |acc, v| acc.max(v),
                        f64::NEG_INFINITY,
                    );
                    stack.push(max);
                }

                Opcode::ArrayMin {} => {
                    let view = view_stack.last().unwrap();
                    let min = Self::reduce_view(
                        temp_storage,
                        view,
                        curr,
                        context,
                        |acc, v| acc.min(v),
                        f64::INFINITY,
                    );
                    stack.push(min);
                }

                Opcode::ArrayMean {} => {
                    let view = view_stack.last().unwrap();
                    let sum =
                        Self::reduce_view(temp_storage, view, curr, context, |acc, v| acc + v, 0.0);
                    let count = view.size() as f64;
                    stack.push(sum / count);
                }

                Opcode::ArrayStddev {} => {
                    let view = view_stack.last().unwrap();
                    let size = view.size();
                    let sum =
                        Self::reduce_view(temp_storage, view, curr, context, |acc, v| acc + v, 0.0);
                    let mean = sum / size as f64;

                    // Second pass for variance
                    let variance_sum = Self::reduce_view(
                        temp_storage,
                        view,
                        curr,
                        context,
                        |acc, v| acc + (v - mean).powi(2),
                        0.0,
                    );
                    let stddev = (variance_sum / size as f64).sqrt();
                    stack.push(stddev);
                }

                Opcode::ArraySize {} => {
                    let view = view_stack.last().unwrap();
                    stack.push(view.size() as f64);
                }

                // =========================================================
                // BROADCASTING ITERATION
                // =========================================================
                Opcode::BeginBroadcastIter {
                    n_sources,
                    dest_temp_id,
                } => {
                    let n = *n_sources as usize;

                    // Collect source views and their view stack indices
                    let source_indices: SmallVec<[usize; 4]> =
                        (view_stack.len() - n..view_stack.len()).collect();

                    // Compute result dimensions by unioning all source dim_ids
                    // We iterate over all dimensions from all sources and build a map
                    let mut result_dim_ids: SmallVec<[DimId; 4]> = SmallVec::new();
                    let mut result_dims: SmallVec<[u16; 4]> = SmallVec::new();

                    for &idx in &source_indices {
                        let view = &view_stack[idx];
                        for (d, &dim_id) in view.dim_ids.iter().enumerate() {
                            if !result_dim_ids.contains(&dim_id) {
                                result_dim_ids.push(dim_id);
                                result_dims.push(view.dims[d]);
                            }
                        }
                    }

                    // For each source, compute dim_map: result dim index -> source dim index (or -1)
                    let mut sources: SmallVec<[BroadcastSourceInfo; 2]> = SmallVec::new();
                    for &idx in &source_indices {
                        let view = &view_stack[idx];
                        let mut dim_map: SmallVec<[i8; 4]> = SmallVec::new();

                        for &result_dim_id in &result_dim_ids {
                            // Find this dim_id in the source
                            if let Some(pos) =
                                view.dim_ids.iter().position(|&id| id == result_dim_id)
                            {
                                dim_map.push(pos as i8);
                            } else {
                                dim_map.push(-1); // Broadcast: source doesn't have this dim
                            }
                        }

                        sources.push(BroadcastSourceInfo {
                            view_stack_idx: idx,
                            dim_map,
                        });
                    }

                    // Compute total size
                    let size = result_dims.iter().map(|&d| d as usize).product();

                    broadcast_stack.push(BroadcastState {
                        sources,
                        dest_temp_id: *dest_temp_id,
                        result_dims,
                        result_indices: smallvec::smallvec![0; result_dim_ids.len()],
                        current: 0,
                        size,
                    });
                }

                Opcode::LoadBroadcastElement { source_idx } => {
                    let bc_state = broadcast_stack.last().unwrap();
                    let source_info = &bc_state.sources[*source_idx as usize];
                    let view = &view_stack[source_info.view_stack_idx];

                    // Return NaN for invalid views
                    if !view.is_valid {
                        stack.push(f64::NAN);
                    } else {
                        // Map result indices to source indices
                        let mut source_indices: SmallVec<[u16; 4]> = SmallVec::new();
                        for (result_dim, &src_dim) in source_info.dim_map.iter().enumerate() {
                            if src_dim >= 0 {
                                // This result dimension maps to source dimension src_dim
                                // But we need to put it in the source's dimension order
                                source_indices.push(bc_state.result_indices[result_dim]);
                            }
                        }

                        // Reorder source_indices according to source's original dim order
                        let mut ordered_source_indices: SmallVec<[u16; 4]> =
                            smallvec::smallvec![0; view.dims.len()];
                        for (result_dim, &src_dim) in source_info.dim_map.iter().enumerate() {
                            if src_dim >= 0 {
                                ordered_source_indices[src_dim as usize] =
                                    bc_state.result_indices[result_dim];
                            }
                        }

                        let flat_off = view.flat_offset(&ordered_source_indices);

                        let value = if view.is_temp {
                            let temp_off = context.temp_offsets[view.base_off as usize];
                            temp_storage[temp_off + flat_off]
                        } else {
                            curr[view.base_off as usize + flat_off]
                        };
                        stack.push(value);
                    }
                }

                Opcode::StoreBroadcastElement {} => {
                    let value = stack.pop();
                    let bc_state = broadcast_stack.last().unwrap();
                    let temp_off = context.temp_offsets[bc_state.dest_temp_id as usize];
                    temp_storage[temp_off + bc_state.current] = value;
                }

                Opcode::NextBroadcastOrJump { jump_back } => {
                    let bc_state = broadcast_stack.last_mut().unwrap();
                    bc_state.current += 1;

                    if bc_state.current < bc_state.size {
                        // Increment result indices (row-major order)
                        let n_dims = bc_state.result_dims.len();
                        for d in (0..n_dims).rev() {
                            bc_state.result_indices[d] += 1;
                            if bc_state.result_indices[d] < bc_state.result_dims[d] {
                                break;
                            }
                            bc_state.result_indices[d] = 0;
                        }

                        // Jump backward to loop start
                        pc = (pc as isize + *jump_back as isize) as usize;
                        continue; // Skip pc increment
                    }
                    // else: iteration done, continue to next opcode
                }

                Opcode::EndBroadcastIter {} => {
                    broadcast_stack.pop();
                }
            }

            pc += 1;
        }
    }

    /// Helper: Reduce all elements of a view using a fold function
    fn reduce_view<F>(
        temp_storage: &[f64],
        view: &RuntimeView,
        curr: &[f64],
        context: &ByteCodeContext,
        f: F,
        init: f64,
    ) -> f64
    where
        F: Fn(f64, f64) -> f64,
    {
        // Return NaN for invalid views
        if !view.is_valid {
            return f64::NAN;
        }

        let size = view.size();
        let dims = &view.dims;
        let n_dims = dims.len();

        let mut acc = init;
        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];

        for _ in 0..size {
            let flat_off = view.flat_offset(&indices);

            let value = if view.is_temp {
                let temp_off = context.temp_offsets[view.base_off as usize];
                temp_storage[temp_off + flat_off]
            } else {
                curr[view.base_off as usize + flat_off]
            };

            acc = f(acc, value);

            // Increment indices (row-major order)
            for d in (0..n_dims).rev() {
                indices[d] += 1;
                if indices[d] < dims[d] {
                    break;
                }
                indices[d] = 0;
            }
        }

        acc
    }

    #[cfg(test)]
    pub fn debug_print_bytecode(&self, _model_name: &str) {
        let mut module_keys: Vec<_> = self.sliced_sim.initial_modules.keys().collect();
        module_keys.sort_unstable();
        for module_key in module_keys {
            eprintln!("\n\nCOMPILED MODULE: {:?}", module_key);

            let initial_bc = &self.sliced_sim.initial_modules[module_key].bytecode;
            let flows_bc = &self.sliced_sim.flow_modules[module_key].bytecode;
            let stocks_bc = &self.sliced_sim.stock_modules[module_key].bytecode;

            eprintln!("\ninitial literals:");
            for (i, lit) in initial_bc.literals.iter().enumerate() {
                eprintln!("\t{i}: {lit}");
            }

            eprintln!("\ninital bytecode:");
            for op in initial_bc.code.iter() {
                eprintln!("\t{op:?}");
            }

            eprintln!("\nflows literals:");
            for (i, lit) in flows_bc.literals.iter().enumerate() {
                eprintln!("\t{i}: {lit}");
            }

            eprintln!("\nflows bytecode:");
            for op in flows_bc.code.iter() {
                eprintln!("\t{op:?}");
            }

            eprintln!("\nstocks literals:");
            for (i, lit) in stocks_bc.literals.iter().enumerate() {
                eprintln!("\t{i}: {lit}");
            }

            eprintln!("\nstocks bytecode:");
            for op in stocks_bc.code.iter() {
                eprintln!("\t{op:?}");
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
        BuiltinId::Inf => f64::INFINITY,
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
        BuiltinId::Sign => {
            if a > 0.0 {
                1.0
            } else if a < 0.0 {
                -1.0
            } else {
                0.0
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
    use crate::datamodel;
    let empty_dim = Dimension::from(datamodel::Dimension::named("".to_string(), vec![]));
    let one_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned()],
    ));
    let two_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned()],
    ));
    let three_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    ));
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
    type Item = Vec<String>;

    fn next(&mut self) -> Option<Vec<String>> {
        self.offsets.next().map(|subscripts| {
            subscripts
                .iter()
                .enumerate()
                .map(|(i, elem)| match &self.dims[i] {
                    Dimension::Named(_, named_dim) => {
                        named_dim.elements[*elem].as_str().to_string()
                    }
                    Dimension::Indexed(_name, _size) => format!("{}", elem + 1),
                })
                .collect()
        })
    }
}

#[test]
fn test_subscript_iter() {
    use crate::datamodel;
    let empty_dim = Dimension::from(datamodel::Dimension::named("".to_string(), vec![]));
    let one_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned()],
    ));
    let two_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned()],
    ));
    let three_dim = Dimension::from(datamodel::Dimension::named(
        "".to_string(),
        vec!["0".to_owned(), "1".to_owned(), "2".to_owned()],
    ));
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
        let mut n = 0;
        for (i, subscripts) in SubscriptIterator::new(input).enumerate() {
            let refs: Vec<&str> = subscripts.iter().map(|s| s.as_str()).collect();
            eprintln!("exp: {expected:?}", expected = expected[i]);
            eprintln!("got: {refs:?}");
            assert_eq!(expected[i], refs);
            n += 1;
        }
        assert_eq!(expected.len(), n);
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

/// Step function lookup that returns the y-value of the next point >= x.
/// If x is beyond the last point, returns the y-value of the last point.
/// This is a "sample and hold" interpolation where we look forward.
#[inline(never)]
fn lookup_forward(table: &[(f64, f64)], index: f64) -> f64 {
    if table.is_empty() {
        return f64::NAN;
    }

    if index.is_nan() {
        return f64::NAN;
    }

    // If index is at or below the first point, return first y
    if index <= table[0].0 {
        return table[0].1;
    }

    // If index is at or above the last point, return last y
    let size = table.len();
    if index >= table[size - 1].0 {
        return table[size - 1].1;
    }

    // Binary search for the first point with x >= index
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

    // low now points to the first element >= index
    table[low].1
}

/// Step function lookup that returns the y-value of the last point where x <= index.
/// If x is before the first point, returns the y-value of the first point.
/// This is a "sample and hold" interpolation where we look backward.
///
/// For duplicate x-values, returns the y of the LAST point with that x.
#[inline(never)]
fn lookup_backward(table: &[(f64, f64)], index: f64) -> f64 {
    if table.is_empty() {
        return f64::NAN;
    }

    if index.is_nan() {
        return f64::NAN;
    }

    // If index is at or below the first point, return first y
    if index <= table[0].0 {
        return table[0].1;
    }

    // If index is at or above the last point, return last y
    let size = table.len();
    if index >= table[size - 1].0 {
        return table[size - 1].1;
    }

    // Binary search for the first point with x > index (upper bound)
    // This gives us the insertion point after all elements <= index
    let mut low = 0;
    let mut high = size;
    while low < high {
        let mid = low + (high - low) / 2;
        if table[mid].0 <= index {
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    // low now points to the first element > index
    // We want the element just before it (the last element <= index)
    table[low - 1].1
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    // Table: (0,0), (1,1), (2,2)
    fn test_table() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]
    }

    #[test]
    fn test_lookup_forward_between_points() {
        let table = test_table();
        // Between (0,0) and (1,1), forward should return 1
        assert_eq!(1.0, lookup_forward(&table, 0.5));
        // Between (1,1) and (2,2), forward should return 2
        assert_eq!(2.0, lookup_forward(&table, 1.5));
    }

    #[test]
    fn test_lookup_forward_at_points() {
        let table = test_table();
        assert_eq!(0.0, lookup_forward(&table, 0.0));
        assert_eq!(1.0, lookup_forward(&table, 1.0));
        assert_eq!(2.0, lookup_forward(&table, 2.0));
    }

    #[test]
    fn test_lookup_forward_outside_range() {
        let table = test_table();
        // Below min: return first y
        assert_eq!(0.0, lookup_forward(&table, -1.0));
        // Above max: return last y
        assert_eq!(2.0, lookup_forward(&table, 2.5));
    }

    #[test]
    fn test_lookup_backward_between_points() {
        let table = test_table();
        // Between (0,0) and (1,1), backward should return 0
        assert_eq!(0.0, lookup_backward(&table, 0.5));
        // Between (1,1) and (2,2), backward should return 1
        assert_eq!(1.0, lookup_backward(&table, 1.5));
    }

    #[test]
    fn test_lookup_backward_at_points() {
        let table = test_table();
        assert_eq!(0.0, lookup_backward(&table, 0.0));
        assert_eq!(1.0, lookup_backward(&table, 1.0));
        assert_eq!(2.0, lookup_backward(&table, 2.0));
    }

    #[test]
    fn test_lookup_backward_outside_range() {
        let table = test_table();
        // Below min: return first y
        assert_eq!(0.0, lookup_backward(&table, -1.0));
        // Above max: return last y
        assert_eq!(2.0, lookup_backward(&table, 2.5));
    }

    #[test]
    fn test_lookup_empty_table() {
        let table: Vec<(f64, f64)> = vec![];
        assert!(lookup_forward(&table, 0.5).is_nan());
        assert!(lookup_backward(&table, 0.5).is_nan());
    }

    #[test]
    fn test_lookup_nan_index() {
        let table = test_table();
        assert!(lookup_forward(&table, f64::NAN).is_nan());
        assert!(lookup_backward(&table, f64::NAN).is_nan());
    }

    #[test]
    fn test_regular_lookup_interpolates() {
        let table = test_table();
        // Regular lookup should interpolate
        assert_eq!(0.5, lookup(&table, 0.5));
        assert_eq!(1.5, lookup(&table, 1.5));
    }
}
