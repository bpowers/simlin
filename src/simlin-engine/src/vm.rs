// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use float_cmp::approx_eq;
use smallvec::SmallVec;

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledInitial, CompiledModule, DimId, LookupMode,
    ModuleId, Op2, Opcode, RuntimeView, TempId,
};
use crate::common::{Canonical, Ident, Result};
use crate::dimensions::{Dimension, match_dimensions_two_pass};
#[allow(unused_imports)]
pub use crate::results::{Method, Results, Specs};
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct BroadcastSourceInfo {
    /// Index into view_stack for this source
    view_stack_idx: usize,
    /// For each result dimension, which source dimension it maps to.
    /// -1 means this source doesn't have this dimension (broadcast).
    dim_map: SmallVec<[i8; 4]>,
}

/// State for broadcast iteration over multiple sources.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<ModuleKey, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: ModuleKey,
    pub(crate) offsets: HashMap<Ident<Canonical>, usize>,
}

/// Per-module compiled initials with the shared ByteCodeContext needed to eval them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct CompiledModuleInitials {
    #[allow(dead_code)]
    ident: Ident<Canonical>,
    context: Arc<ByteCodeContext>,
    initials: Arc<Vec<CompiledInitial>>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct CompiledSlicedSimulation {
    initial_modules: HashMap<ModuleKey, CompiledModuleInitials>,
    flow_modules: HashMap<ModuleKey, CompiledModuleSlice>,
    stock_modules: HashMap<ModuleKey, CompiledModuleSlice>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepPart {
    Initials,
    Flows,
    Stocks,
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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
    // Override values: offset -> value. Applied after each variable's
    // initial bytecode executes (evaluate-then-patch).
    overrides: HashMap<usize, f64>,
    // All absolute data-buffer offsets that are written during initials
    // (precomputed from the module tree for fast override validation).
    initial_offsets: std::collections::HashSet<usize>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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
                StepPart::Flows => module.compiled_flows.clone(),
                StepPart::Stocks => module.compiled_stocks.clone(),
                StepPart::Initials => unreachable!("initials use CompiledModuleInitials"),
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

        // Precompute all absolute initial offsets by walking the module tree
        let initial_offsets = Self::collect_initial_offsets(&sim.modules, &sim.root, 0);

        Ok(Vm {
            specs: sim.specs,
            root: sim.root,
            offsets: sim.offsets,
            sliced_sim: CompiledSlicedSimulation {
                initial_modules: sim
                    .modules
                    .iter()
                    .map(|(id, m)| {
                        (
                            id.clone(),
                            CompiledModuleInitials {
                                ident: m.ident.clone(),
                                context: m.context.clone(),
                                initials: m.compiled_initials.clone(),
                            },
                        )
                    })
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
            overrides: HashMap::new(),
            initial_offsets,
        })
    }

    /// Recursively collect all absolute initial offsets from a module and its submodules.
    fn collect_initial_offsets(
        modules: &HashMap<ModuleKey, CompiledModule>,
        module_key: &ModuleKey,
        base_off: usize,
    ) -> std::collections::HashSet<usize> {
        let mut result = std::collections::HashSet::new();
        let module = &modules[module_key];

        // Add this module's initial offsets, translated to absolute
        for ci in module.compiled_initials.iter() {
            for &off in &ci.offsets {
                result.insert(base_off + off);
            }
        }

        // Recurse into submodules
        for module_decl in &module.context.modules {
            let child_key = make_module_key(&module_decl.model_name, &module_decl.input_set);
            let child_base = base_off + module_decl.off;
            result.extend(Self::collect_initial_offsets(
                modules, &child_key, child_base,
            ));
        }

        result
    }

    pub fn run_to_end(&mut self) -> Result<()> {
        let end = self.specs.stop;
        self.run_to(end)
    }

    #[inline(never)]
    pub fn run_to(&mut self, end: f64) -> Result<()> {
        self.run_initials()?;

        let spec_start = self.specs.start;
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

    /// Returns whether a given absolute data-buffer offset is written during
    /// initials evaluation (O(1) lookup against precomputed set).
    fn is_initial_offset(&self, off: usize) -> bool {
        self.initial_offsets.contains(&off)
    }

    /// Reset the VM to its pre-simulation state, reusing the data buffer allocation.
    /// Overrides are preserved across reset.
    pub fn reset(&mut self) {
        if let Some(ref mut data) = self.data {
            data.fill(0.0);
        }
        self.curr_chunk = 0;
        self.next_chunk = 1;
        self.did_initials = false;
        self.step_accum = 0;
        self.temp_storage.fill(0.0);
    }

    /// Set an override by canonical variable name. The override value will be
    /// patched into the data buffer after the variable's initial bytecode executes.
    pub fn set_override(&mut self, ident: &Ident<Canonical>, value: f64) -> Result<()> {
        let off = match self.offsets.get(ident) {
            Some(&off) => off,
            None => {
                return sim_err!(
                    DoesNotExist,
                    format!("variable '{}' not found in offsets map", ident.as_str())
                );
            }
        };
        if !self.is_initial_offset(off) {
            return sim_err!(
                BadSimSpecs,
                format!(
                    "cannot override '{}': not an initial variable",
                    ident.as_str()
                )
            );
        }
        self.overrides.insert(off, value);
        Ok(())
    }

    /// Set an override by raw data-buffer offset.
    pub fn set_override_by_offset(&mut self, off: usize, value: f64) -> Result<()> {
        if off >= self.n_slots {
            return sim_err!(
                BadSimSpecs,
                format!("offset {} out of bounds (n_slots={})", off, self.n_slots)
            );
        }
        if !self.is_initial_offset(off) {
            return sim_err!(
                BadSimSpecs,
                format!("cannot override offset {}: not an initial variable", off)
            );
        }
        self.overrides.insert(off, value);
        Ok(())
    }

    /// Remove all overrides.
    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    /// Run only the initials phase (idempotent: no-op if already done).
    /// After this call, chunk 0 contains the t=0 state.
    pub fn run_initials(&mut self) -> Result<()> {
        if self.did_initials {
            return Ok(());
        }

        let spec_start = self.specs.start;
        let spec_stop = self.specs.stop;
        let dt = self.specs.dt;

        let mut stack = Stack::new();
        let module_inputs: &[f64] = &[0.0; 0];
        let mut data = None;
        std::mem::swap(&mut data, &mut self.data);
        let mut data = data.unwrap();

        let (curr, next) = borrow_two(&mut data, self.n_slots, self.curr_chunk, self.next_chunk);
        curr[TIME_OFF] = spec_start;
        curr[DT_OFF] = dt;
        curr[INITIAL_TIME_OFF] = spec_start;
        curr[FINAL_TIME_OFF] = spec_stop;

        Self::eval_initials_with_overrides(
            &self.sliced_sim,
            &mut self.temp_storage,
            &self.root,
            0,
            module_inputs,
            curr,
            next,
            &mut stack,
            &self.overrides,
        );
        self.did_initials = true;
        self.step_accum = 0;

        self.data = Some(data);
        Ok(())
    }

    /// Extract the time series for a variable after simulation.
    /// Returns None if the ident is not found.
    /// The returned vector has one element per saved step (including t=0).
    pub fn get_series(&self, ident: &Ident<Canonical>) -> Option<Vec<f64>> {
        let &off = self.offsets.get(ident)?;
        let data = self.data.as_ref()?;
        if !self.did_initials {
            return Some(vec![]);
        }
        // After the main loop, curr_chunk equals the number of valid
        // saved steps (e.g. 101 for a 0..100 run).  After run_initials()
        // alone, curr_chunk is still 0 but chunk 0 is valid (1 step).
        let n_steps = if self.curr_chunk == 0 {
            1
        } else {
            std::cmp::min(self.curr_chunk, self.n_chunks)
        };
        let mut series = Vec::with_capacity(n_steps);
        for chunk_idx in 0..n_steps {
            let base = chunk_idx * self.n_slots;
            series.push(data[base + off]);
        }
        Some(series)
    }

    /// Evaluate a submodule's initials (all per-variable CompiledInitials),
    /// applying overrides after each variable.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn eval_module_initials_with_overrides(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        parent_context: &ByteCodeContext,
        parent_module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        id: ModuleId,
        overrides: &HashMap<usize, f64>,
    ) {
        let new_module_decl = &parent_context.modules[id as usize];
        let module_key = make_module_key(&new_module_decl.model_name, &new_module_decl.input_set);
        let module_off = parent_module_off + new_module_decl.off;

        Self::eval_initials_with_overrides(
            sliced_sim,
            temp_storage,
            &module_key,
            module_off,
            module_inputs,
            curr,
            next,
            stack,
            overrides,
        );
    }

    /// Run all per-variable initials for a module (in dependency order),
    /// applying overrides after each variable's bytecode completes.
    #[allow(clippy::too_many_arguments)]
    fn eval_initials_with_overrides(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        module_key: &ModuleKey,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        overrides: &HashMap<usize, f64>,
    ) {
        let module_initials = &sliced_sim.initial_modules[module_key];
        let context = &module_initials.context;
        for compiled_initial in module_initials.initials.iter() {
            Self::eval_single_initial(
                sliced_sim,
                temp_storage,
                context,
                &compiled_initial.bytecode,
                module_off,
                module_inputs,
                curr,
                next,
                stack,
                overrides,
            );
            // Evaluate-then-patch: apply overrides after bytecode completes.
            // CompiledInitial offsets are module-relative; add module_off
            // to get the absolute position in the flattened data buffer.
            for &off in &compiled_initial.offsets {
                let abs_off = module_off + off;
                if let Some(&val) = overrides.get(&abs_off) {
                    curr[abs_off] = val;
                }
            }
        }
    }

    /// Evaluate a single variable's initial bytecode.
    #[allow(clippy::too_many_arguments)]
    fn eval_single_initial(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        context: &ByteCodeContext,
        bytecode: &ByteCode,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        overrides: &HashMap<usize, f64>,
    ) {
        Self::eval_bytecode(
            sliced_sim,
            temp_storage,
            context,
            bytecode,
            StepPart::Initials,
            module_off,
            module_inputs,
            curr,
            next,
            stack,
            overrides,
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
        static EMPTY_OVERRIDES: std::sync::LazyLock<HashMap<usize, f64>> =
            std::sync::LazyLock::new(HashMap::new);
        Self::eval_bytecode(
            sliced_sim,
            temp_storage,
            &module.context,
            &module.bytecode,
            module.part,
            module_off,
            module_inputs,
            curr,
            next,
            stack,
            &EMPTY_OVERRIDES,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_bytecode(
        sliced_sim: &CompiledSlicedSimulation,
        temp_storage: &mut [f64],
        context: &ByteCodeContext,
        bytecode: &ByteCode,
        part: StepPart,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        stack: &mut Stack,
        overrides: &HashMap<usize, f64>,
    ) {
        // Existing state
        let mut condition = false;
        let mut subscript_index: SmallVec<[(u16, u16); 4]> = SmallVec::new();
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
                    match part {
                        StepPart::Initials => {
                            Self::eval_module_initials_with_overrides(
                                sliced_sim,
                                temp_storage,
                                context,
                                module_off,
                                &module_inputs,
                                curr,
                                next,
                                stack,
                                *id,
                                overrides,
                            );
                        }
                        StepPart::Flows | StepPart::Stocks => {
                            let new_module_decl = &context.modules[*id as usize];
                            let module_key = make_module_key(
                                &new_module_decl.model_name,
                                &new_module_decl.input_set,
                            );
                            let child_module_off = module_off + new_module_decl.off;
                            let child_module = match part {
                                StepPart::Flows => &sliced_sim.flow_modules[&module_key],
                                StepPart::Stocks => &sliced_sim.stock_modules[&module_key],
                                StepPart::Initials => unreachable!(),
                            };
                            Self::eval(
                                sliced_sim,
                                temp_storage,
                                child_module,
                                child_module_off,
                                &module_inputs,
                                curr,
                                next,
                                stack,
                            );
                        }
                    }
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

                Opcode::LoadIterViewAt { offset } => {
                    // Like LoadIterViewTop but accesses a view at a specific stack offset.
                    // offset=1 means top of stack, offset=2 means second from top, etc.
                    // This allows views to be pushed before the loop and accessed inside
                    // without repeated push/pop operations per iteration.
                    let iter_state = iter_stack.last().unwrap();
                    let source_view_idx = view_stack.len() - *offset as usize;
                    let source_view = &view_stack[source_view_idx];

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

            let module_initials = &self.sliced_sim.initial_modules[module_key];
            let flows_bc = &self.sliced_sim.flow_modules[module_key].bytecode;
            let stocks_bc = &self.sliced_sim.stock_modules[module_key].bytecode;

            for ci in module_initials.initials.iter() {
                eprintln!("\ninitial '{}' literals:", ci.ident);
                for (i, lit) in ci.bytecode.literals.iter().enumerate() {
                    eprintln!("\t{i}: {lit}");
                }
                eprintln!(
                    "initial '{}' bytecode (offsets {:?}):",
                    ci.ident, ci.offsets
                );
                for op in ci.bytecode.code.iter() {
                    eprintln!("\t{op:?}");
                }
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

#[cfg(test)]
mod per_variable_initials_tests {
    use super::*;
    use crate::test_common::TestProject;

    /// Helper: build a Simulation and CompiledSimulation from a TestProject
    fn build_compiled(tp: &TestProject) -> (crate::interpreter::Simulation, CompiledSimulation) {
        let sim = tp.build_sim().expect("build_sim failed");
        let compiled = sim.compile().expect("compile failed");
        (sim, compiled)
    }

    #[test]
    fn test_per_var_initials_matches_interpreter() {
        let tp = TestProject::new("per_var_test")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("rate", "0.1", None)
            .aux("scaled_rate", "rate * 10", None)
            .flow("births", "population * rate", None)
            .flow("deaths", "population / 80", None)
            .stock("population", "100", &["births"], &["deaths"], None);

        let interp_results = tp
            .run_interpreter()
            .expect("interpreter should run successfully");
        let vm_results = tp.run_vm().expect("VM should run successfully");

        let pop_ident = "population";
        let interp_pop = &interp_results[pop_ident];
        let vm_pop = &vm_results[pop_ident];

        assert_eq!(
            interp_pop.len(),
            vm_pop.len(),
            "step count should match between interpreter and VM"
        );
        for (i, (interp_val, vm_val)) in interp_pop.iter().zip(vm_pop.iter()).enumerate() {
            assert!(
                (interp_val - vm_val).abs() < 1e-10,
                "population mismatch at step {i}: interpreter={interp_val}, vm={vm_val}"
            );
        }
    }

    #[test]
    fn test_per_var_initials_dependency_order() {
        // a = 5, b = a * 2, c = b + 1, stock initial = c
        let tp = TestProject::new("dep_order_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "5", None)
            .aux("b", "a * 2", None)
            .aux("c", "b + 1", None)
            .flow("inflow", "0", None)
            .stock("s", "c", &["inflow"], &[], None);

        let vm_results = tp.run_vm().expect("VM should succeed");
        let interp_results = tp.run_interpreter().expect("interpreter should succeed");

        // Check initial values (step 0)
        let s_vm = &vm_results["s"];
        let s_interp = &interp_results["s"];
        assert_eq!(s_vm[0], 11.0, "stock initial = c = b+1 = a*2+1 = 11 (VM)");
        assert_eq!(
            s_interp[0], 11.0,
            "stock initial = c = b+1 = a*2+1 = 11 (interpreter)"
        );
    }

    #[test]
    fn test_per_var_initials_with_module() {
        let test_file = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx"
        );
        let file_bytes =
            std::fs::read(test_file).expect("modules_hares_and_foxes test fixture must exist");
        let mut cursor = std::io::Cursor::new(file_bytes);
        let project_datamodel = crate::open_xmile(&mut cursor).unwrap();
        let project = std::sync::Arc::new(crate::project::Project::from(project_datamodel));
        let sim =
            crate::interpreter::Simulation::new(&project, "main").expect("Simulation should build");

        let interp_results = sim.run_to_end().expect("interpreter run should succeed");
        let compiled = sim.compile().expect("compile should succeed");
        let mut vm = Vm::new(compiled).expect("VM creation should succeed");
        vm.run_to_end().expect("VM run should succeed");
        let vm_results = vm.into_results();

        // Compare all offsets between interpreter and VM at every timestep
        for (name, &offset) in &interp_results.offsets {
            for step in 0..std::cmp::min(interp_results.step_count, vm_results.step_count) {
                let idx = step * interp_results.step_size + offset;
                let interp_val = interp_results.data[idx];
                let vm_val = vm_results.data[idx];
                assert!(
                    (interp_val - vm_val).abs() < 1e-10 || (interp_val.is_nan() && vm_val.is_nan()),
                    "mismatch for {name} at step {step}: interpreter={interp_val}, vm={vm_val}"
                );
            }
        }
    }

    #[test]
    fn test_compiled_initial_offsets_sorted_deduped() {
        // Use a model where auxiliary 'rate' is a stock dependency so it
        // appears in the initials runlist.
        let tp = TestProject::new("offsets_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "0", None)
            .stock("pop", "rate * 1000", &["inflow"], &[], None);

        let (_, compiled) = build_compiled(&tp);
        let root_key = &compiled.root;
        let root_module = &compiled.modules[root_key];

        // Verify that CompiledInitial offsets are sorted and unique
        for ci in root_module.compiled_initials.iter() {
            let offsets = &ci.offsets;
            for window in offsets.windows(2) {
                assert!(
                    window[0] < window[1],
                    "offsets for '{}' should be sorted and unique: {:?}",
                    ci.ident,
                    offsets
                );
            }
        }

        // Verify that each CompiledInitial has a non-empty ident
        for ci in root_module.compiled_initials.iter() {
            assert!(
                !ci.ident.as_str().is_empty(),
                "CompiledInitial should have a non-empty ident"
            );
        }

        // The initials should include 'rate' (stock depends on it) and 'pop'
        let idents: Vec<&str> = root_module
            .compiled_initials
            .iter()
            .map(|ci| ci.ident.as_str())
            .collect();
        assert!(
            idents.contains(&"rate"),
            "should have 'rate' initial (stock depends on it), got: {:?}",
            idents
        );
        assert!(
            idents.contains(&"pop"),
            "should have 'pop' initial, got: {:?}",
            idents
        );

        // Verify the stock's initial value is correct: rate * 1000 = 100
        let vm_results = tp.run_vm().expect("VM should succeed");
        let pop_vm = &vm_results["pop"];
        assert_eq!(pop_vm[0], 100.0, "population initial should be 100");
    }

    #[test]
    fn test_per_var_initials_with_array() {
        let tp = TestProject::new("array_init_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("Dim", &["A", "B", "C"])
            .array_with_ranges("arr[Dim]", vec![("A", "1"), ("B", "2"), ("C", "3")])
            .flow("inflow", "0", None)
            .stock("s", "arr[A] + arr[B] + arr[C]", &["inflow"], &[], None);

        let interp_results = tp.run_interpreter().expect("interpreter should succeed");
        let vm_results = tp.run_vm().expect("VM should succeed");

        // arr[A]=1, arr[B]=2, arr[C]=3, so s = 1+2+3 = 6
        let s_interp = interp_results
            .get("s")
            .expect("s should exist in interpreter");
        let s_vm = vm_results.get("s").expect("s should exist in VM");
        assert_eq!(s_interp[0], 6.0, "s initial = 6 in interpreter");
        assert_eq!(s_vm[0], 6.0, "s initial = 6 in VM");

        // Verify individual array elements match (names are canonicalized to lowercase)
        for element in &["arr[a]", "arr[b]", "arr[c]"] {
            let interp_val = interp_results
                .get(*element)
                .unwrap_or_else(|| panic!("{element} should exist in interpreter results"));
            let vm_val = vm_results
                .get(*element)
                .unwrap_or_else(|| panic!("{element} should exist in VM results"));
            assert!(
                (interp_val[0] - vm_val[0]).abs() < 1e-10,
                "{element}: interpreter={}, vm={}",
                interp_val[0],
                vm_val[0]
            );
        }

        // Verify CompiledInitial offsets for the array variable
        let (_, compiled) = build_compiled(&tp);
        let root_module = &compiled.modules[&compiled.root];
        let arr_initial = root_module
            .compiled_initials
            .iter()
            .find(|ci| ci.ident.as_str() == "arr")
            .expect("should have 'arr' CompiledInitial");

        assert_eq!(
            arr_initial.offsets.len(),
            3,
            "arr should have 3 offsets (one per element)"
        );
        // Offsets should be contiguous
        assert_eq!(
            arr_initial.offsets[1] - arr_initial.offsets[0],
            1,
            "array offsets should be contiguous"
        );
        assert_eq!(
            arr_initial.offsets[2] - arr_initial.offsets[1],
            1,
            "array offsets should be contiguous"
        );
    }
}

#[cfg(test)]
mod vm_reset_and_run_initials_tests {
    use super::*;
    use crate::canonicalize;
    use crate::test_common::TestProject;

    fn pop_model() -> TestProject {
        TestProject::new("pop_model")
            .with_sim_time(0.0, 100.0, 1.0)
            .aux("birth_rate", "0.1", None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population / 80", None)
            .stock("population", "100", &["births"], &["deaths"], None)
    }

    fn build_compiled(tp: &TestProject) -> (crate::interpreter::Simulation, CompiledSimulation) {
        let sim = tp.build_sim().unwrap();
        let compiled = sim.compile().unwrap();
        (sim, compiled)
    }

    #[test]
    fn test_vm_reset_produces_identical_results() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        // First run
        let mut vm1 = Vm::new(compiled.clone()).unwrap();
        vm1.run_to_end().unwrap();
        let results1 = vm1.into_results();

        // Second fresh VM from same compiled
        let mut vm2 = Vm::new(compiled.clone()).unwrap();
        vm2.run_to_end().unwrap();
        let results2 = vm2.into_results();

        // Third: run, reset, run again
        let mut vm3 = Vm::new(compiled).unwrap();
        vm3.run_to_end().unwrap();
        vm3.reset();
        vm3.run_to_end().unwrap();
        let results3 = vm3.into_results();

        let pop_off = *results1.offsets.get(&canonicalize("population")).unwrap();
        for step in 0..results1.step_count {
            let idx = step * results1.step_size + pop_off;
            let v1 = results1.data[idx];
            let v2 = results2.data[idx];
            let v3 = results3.data[idx];
            assert!(
                (v1 - v2).abs() < 1e-10,
                "fresh VMs should match at step {step}: {v1} vs {v2}"
            );
            assert!(
                (v1 - v3).abs() < 1e-10,
                "reset VM should match fresh at step {step}: {v1} vs {v3}"
            );
        }
    }

    #[test]
    fn test_vm_reset_after_partial_run() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        // Full run for reference
        let mut vm_ref = Vm::new(compiled.clone()).unwrap();
        vm_ref.run_to_end().unwrap();
        let ref_results = vm_ref.into_results();

        // Partial run, then reset and full run
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to(50.0).unwrap();
        vm.reset();
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        let pop_off = *ref_results
            .offsets
            .get(&canonicalize("population"))
            .unwrap();
        for step in 0..ref_results.step_count {
            let idx = step * ref_results.step_size + pop_off;
            let v_ref = ref_results.data[idx];
            let v = results.data[idx];
            assert!(
                (v_ref - v).abs() < 1e-10,
                "reset-after-partial should match fresh at step {step}: {v_ref} vs {v}"
            );
        }
    }

    #[test]
    fn test_compiled_simulation_clone_produces_equivalent_vm() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);
        let compiled_clone = compiled.clone();

        let mut vm1 = Vm::new(compiled).unwrap();
        vm1.run_to_end().unwrap();
        let results1 = vm1.into_results();

        let mut vm2 = Vm::new(compiled_clone).unwrap();
        vm2.run_to_end().unwrap();
        let results2 = vm2.into_results();

        let pop_off = *results1.offsets.get(&canonicalize("population")).unwrap();
        for step in 0..results1.step_count {
            let idx = step * results1.step_size + pop_off;
            assert!(
                (results1.data[idx] - results2.data[idx]).abs() < 1e-10,
                "cloned compiled should produce identical results at step {step}"
            );
        }
    }

    #[test]
    fn test_run_initials_then_run_to_end_matches_single_call() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        // VM A: single run_to_end
        let mut vm_a = Vm::new(compiled.clone()).unwrap();
        vm_a.run_to_end().unwrap();
        let results_a = vm_a.into_results();

        // VM B: run_initials then run_to_end
        let mut vm_b = Vm::new(compiled).unwrap();
        vm_b.run_initials().unwrap();
        vm_b.run_to_end().unwrap();
        let results_b = vm_b.into_results();

        let pop_off = *results_a.offsets.get(&canonicalize("population")).unwrap();
        for step in 0..results_a.step_count {
            let idx = step * results_a.step_size + pop_off;
            assert!(
                (results_a.data[idx] - results_b.data[idx]).abs() < 1e-10,
                "run_initials+run_to_end should match single run_to_end at step {step}"
            );
        }
    }

    #[test]
    fn test_run_initials_is_idempotent() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();
        vm.run_initials().unwrap(); // second call should be no-op
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        let pop_off = *results.offsets.get(&canonicalize("population")).unwrap();
        let initial_pop = results.data[pop_off];
        assert_eq!(initial_pop, 100.0, "population initial should be 100");
    }

    #[test]
    fn test_run_initials_sets_correct_values() {
        // Use a model where the aux is a stock dependency so it's in initials
        let tp = TestProject::new("initials_check")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "0", None)
            .stock("s", "rate * 1000", &["inflow"], &[], None);

        let (_, compiled) = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();

        let s_off = vm.get_offset(&canonicalize("s")).unwrap();
        let rate_off = vm.get_offset(&canonicalize("rate")).unwrap();

        assert_eq!(
            vm.get_value_now(s_off),
            100.0,
            "stock initial = rate*1000 = 100"
        );
        assert_eq!(
            vm.get_value_now(rate_off),
            0.1,
            "rate is a stock dependency, so it's in initials"
        );
        assert_eq!(
            vm.get_value_now(TIME_OFF),
            0.0,
            "time should be 0 after initials"
        );
    }

    #[test]
    fn test_get_series_after_run_to_end() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        let series = vm.get_series(&canonicalize("population")).unwrap();
        // With start=0, stop=100, save_step=1: 101 steps (0,1,...,100)
        assert_eq!(series.len(), 101, "should have 101 data points");
        assert_eq!(series[0], 100.0, "initial population should be 100");
        // Population should grow (birth_rate > death_rate for pop=100)
        assert!(
            series[100] > series[0],
            "population should grow: final={} > initial={}",
            series[100],
            series[0]
        );
    }

    #[test]
    fn test_get_series_after_partial_run() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to(50.0).unwrap();

        let series = vm.get_series(&canonicalize("population")).unwrap();
        // With start=0, stop=100 but run_to(50): should have 51 steps (0..=50)
        assert_eq!(
            series.len(),
            51,
            "should have 51 data points for run_to(50)"
        );
        assert_eq!(series[0], 100.0, "initial population should be 100");

        // After reset, the VM should still work
        vm.reset();
        vm.run_to_end().unwrap();
        let full_series = vm.get_series(&canonicalize("population")).unwrap();
        assert_eq!(
            full_series.len(),
            101,
            "full run after reset should have 101 points"
        );
    }

    #[test]
    fn test_get_series_after_run_initials_only() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();

        let series = vm.get_series(&canonicalize("population")).unwrap();
        assert_eq!(
            series.len(),
            1,
            "after run_initials only, series should have 1 element"
        );
        assert_eq!(
            series[0], 100.0,
            "the single element should be the initial value"
        );
    }

    #[test]
    fn test_get_series_unknown_variable() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        assert!(
            vm.get_series(&canonicalize("nonexistent_var")).is_none(),
            "unknown variable should return None"
        );
    }

    #[test]
    fn test_get_series_before_any_run() {
        let tp = pop_model();
        let (_, compiled) = build_compiled(&tp);

        let vm = Vm::new(compiled).unwrap();
        let series = vm.get_series(&canonicalize("population")).unwrap();
        assert!(series.is_empty(), "before any run, series should be empty");
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    use crate::canonicalize;
    use crate::test_common::TestProject;

    /// Model: rate=0.1, scaled_rate=rate*10, stock initial=scaled_rate.
    /// `rate` and `scaled_rate` are both stock dependencies in the initials.
    fn rate_model() -> TestProject {
        TestProject::new("rate_model")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("rate", "0.1", None)
            .aux("scaled_rate", "rate * 10", None)
            .flow("inflow", "population * rate", None)
            .flow("outflow", "population / 80", None)
            .stock("population", "scaled_rate", &["inflow"], &["outflow"], None)
    }

    fn build_compiled(tp: &TestProject) -> CompiledSimulation {
        let sim = tp.build_sim().unwrap();
        sim.compile().unwrap()
    }

    #[test]
    fn test_override_constant_flows_through_dependent_initials() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();

        // Override rate from 0.1 to 0.2
        vm.set_override(&canonicalize("rate"), 0.2).unwrap();
        vm.run_initials().unwrap();

        let rate_off = vm.get_offset(&canonicalize("rate")).unwrap();
        let sr_off = vm.get_offset(&canonicalize("scaled_rate")).unwrap();
        let pop_off = vm.get_offset(&canonicalize("population")).unwrap();

        assert_eq!(
            vm.get_value_now(rate_off),
            0.2,
            "rate should be overridden to 0.2"
        );
        assert_eq!(
            vm.get_value_now(sr_off),
            2.0,
            "scaled_rate = rate*10 = 0.2*10 = 2.0"
        );
        assert_eq!(
            vm.get_value_now(pop_off),
            2.0,
            "population initial = scaled_rate = 2.0"
        );
    }

    #[test]
    fn test_override_affects_simulation_results() {
        let compiled = build_compiled(&rate_model());

        // Run without override
        let mut vm1 = Vm::new(compiled.clone()).unwrap();
        vm1.run_to_end().unwrap();
        let series1 = vm1.get_series(&canonicalize("population")).unwrap();

        // Run with override: higher rate means more growth
        let mut vm2 = Vm::new(compiled).unwrap();
        vm2.set_override(&canonicalize("rate"), 0.2).unwrap();
        vm2.run_to_end().unwrap();
        let series2 = vm2.get_series(&canonicalize("population")).unwrap();

        assert!(
            series2.last().unwrap() > series1.last().unwrap(),
            "higher rate should produce higher final population: {} vs {}",
            series2.last().unwrap(),
            series1.last().unwrap()
        );
    }

    #[test]
    fn test_override_persists_across_reset() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();

        vm.set_override(&canonicalize("rate"), 0.2).unwrap();
        vm.run_to_end().unwrap();
        let series_before = vm.get_series(&canonicalize("population")).unwrap();

        vm.reset();
        vm.run_to_end().unwrap();
        let series_after = vm.get_series(&canonicalize("population")).unwrap();

        for (i, (a, b)) in series_before.iter().zip(series_after.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "override should persist across reset: step {i}: {a} vs {b}"
            );
        }
    }

    #[test]
    fn test_clear_overrides_restores_defaults() {
        let compiled = build_compiled(&rate_model());

        // Baseline run
        let mut vm_baseline = Vm::new(compiled.clone()).unwrap();
        vm_baseline.run_to_end().unwrap();
        let baseline = vm_baseline.get_series(&canonicalize("population")).unwrap();

        // Run with override
        let mut vm = Vm::new(compiled).unwrap();
        vm.set_override(&canonicalize("rate"), 0.5).unwrap();
        vm.run_to_end().unwrap();
        let overridden = vm.get_series(&canonicalize("population")).unwrap();

        // Clear and re-run
        vm.clear_overrides();
        vm.reset();
        vm.run_to_end().unwrap();
        let restored = vm.get_series(&canonicalize("population")).unwrap();

        // Overridden should differ from baseline
        assert!(
            (overridden.last().unwrap() - baseline.last().unwrap()).abs() > 1.0,
            "overridden should differ from baseline"
        );
        // Restored should match baseline
        for (i, (b, r)) in baseline.iter().zip(restored.iter()).enumerate() {
            assert!(
                (b - r).abs() < 1e-10,
                "after clear_overrides, should match baseline: step {i}: {b} vs {r}"
            );
        }
    }

    #[test]
    fn test_multiple_reset_override_cycles() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();

        let mut prev_final = 0.0;
        for i in 1..=10 {
            let rate_val = i as f64 * 0.01;
            vm.set_override(&canonicalize("rate"), rate_val).unwrap();
            vm.reset();
            vm.run_to_end().unwrap();
            let series = vm.get_series(&canonicalize("population")).unwrap();
            let final_val = *series.last().unwrap();
            if i > 1 {
                assert!(
                    final_val > prev_final,
                    "final pop should increase with rate: rate={rate_val}, final={final_val}, prev={prev_final}"
                );
            }
            prev_final = final_val;
        }
    }

    #[test]
    fn test_override_nonexistent_variable_returns_error() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();
        let result = vm.set_override(&canonicalize("nonexistent_var"), 1.0);
        assert!(
            result.is_err(),
            "overriding nonexistent variable should fail"
        );
    }

    #[test]
    fn test_override_by_offset_out_of_bounds_returns_error() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();
        let result = vm.set_override_by_offset(99999, 1.0);
        assert!(result.is_err(), "out-of-bounds offset should fail");
    }

    #[test]
    fn test_override_non_initial_variable_returns_error() {
        // birth_rate is not a stock dependency in this model (stock init = "100")
        let tp = TestProject::new("non_initial_override")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("birth_rate", "0.1", None)
            .flow("births", "pop * birth_rate", None)
            .stock("pop", "100", &["births"], &[], None);

        let sim = tp.build_sim().unwrap();
        let compiled = sim.compile().unwrap();
        let mut vm = Vm::new(compiled).unwrap();

        // birth_rate is only used in flows, not in initials
        // It shouldn't be overridable via set_override
        let result = vm.set_override(&canonicalize("birth_rate"), 0.5);
        assert!(
            result.is_err(),
            "overriding a non-initial variable should fail"
        );
    }

    #[test]
    fn test_override_after_initials_requires_reset() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();

        // Run initials first
        vm.run_initials().unwrap();

        // Set override AFTER initials
        vm.set_override(&canonicalize("rate"), 0.5).unwrap();

        // Run to end - override did NOT take effect (initials already done)
        vm.run_to_end().unwrap();
        let series1 = vm.get_series(&canonicalize("population")).unwrap();

        // Now reset and run - override takes effect
        vm.reset();
        vm.run_to_end().unwrap();
        let series2 = vm.get_series(&canonicalize("population")).unwrap();

        // series1 used default rate=0.1, series2 used override rate=0.5
        assert!(
            (series1.last().unwrap() - series2.last().unwrap()).abs() > 1.0,
            "override should take effect only after reset: first={}, second={}",
            series1.last().unwrap(),
            series2.last().unwrap()
        );
    }

    #[test]
    fn test_conflicting_writes_to_same_offset() {
        let compiled = build_compiled(&rate_model());
        let mut vm = Vm::new(compiled).unwrap();

        let rate_off = vm.get_offset(&canonicalize("rate")).unwrap();

        // Two writes to the same offset - last one wins
        vm.set_override_by_offset(rate_off, 0.1).unwrap();
        vm.set_override_by_offset(rate_off, 0.3).unwrap();

        vm.run_initials().unwrap();
        assert_eq!(vm.get_value_now(rate_off), 0.3, "last override should win");
    }

    #[test]
    fn test_override_module_variable() {
        let test_file = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx"
        );
        let file_bytes =
            std::fs::read(test_file).expect("modules_hares_and_foxes test fixture must exist");
        let mut cursor = std::io::Cursor::new(file_bytes);
        let project_datamodel = crate::open_xmile(&mut cursor).unwrap();
        let project = std::sync::Arc::new(crate::project::Project::from(project_datamodel));
        let sim =
            crate::interpreter::Simulation::new(&project, "main").expect("Simulation should build");
        let compiled = sim.compile().unwrap();

        // Run baseline
        let mut vm1 = Vm::new(compiled.clone()).unwrap();
        vm1.run_to_end().unwrap();

        let mut vm2 = Vm::new(compiled).unwrap();
        // calc_flattened_offsets uses from_unchecked with dot separators
        let hares_ident = Ident::<Canonical>::from_unchecked("hares.hares".to_string());
        assert!(
            vm2.get_offset(&hares_ident).is_some(),
            "hares.hares should exist in offsets"
        );
        assert!(
            vm2.is_initial_offset(vm2.offsets[&hares_ident]),
            "hares.hares should be an initial offset"
        );
        vm2.set_override(&hares_ident, 500.0).unwrap();
        vm2.run_to_end().unwrap();
        let s1 = vm1.get_series(&hares_ident).unwrap();
        let s2 = vm2.get_series(&hares_ident).unwrap();
        assert!(
            (s2[0] - 500.0).abs() < 1e-10,
            "overridden initial should be 500, got {}",
            s2[0]
        );
        assert!(
            (s1[0] - s2[0]).abs() > 1.0,
            "override should change initial value: baseline={}, overridden={}",
            s1[0],
            s2[0]
        );
    }

    #[test]
    fn test_override_partial_array() {
        let tp = TestProject::new("array_override")
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("Dim", &["A", "B", "C"])
            .array_with_ranges("arr[Dim]", vec![("A", "1"), ("B", "2"), ("C", "3")])
            .aux("total", "arr[A] + arr[B] + arr[C]", None)
            .flow("inflow", "0", None)
            .stock("s", "total", &["inflow"], &[], None);

        let sim = tp.build_sim().unwrap();
        let compiled = sim.compile().unwrap();
        let mut vm = Vm::new(compiled).unwrap();

        let arr_b_ident = canonicalize("arr[b]");
        let arr_b_off = vm
            .get_offset(&arr_b_ident)
            .expect("arr[b] should exist in offsets");
        vm.set_override_by_offset(arr_b_off, 99.0).unwrap();
        vm.run_initials().unwrap();
        assert_eq!(
            vm.get_value_now(arr_b_off),
            99.0,
            "arr[b] should be overridden to 99"
        );
        let s_off = vm.get_offset(&canonicalize("s")).unwrap();
        // total = arr[A]+arr[B]+arr[C] = 1+99+3 = 103
        assert_eq!(
            vm.get_value_now(s_off),
            103.0,
            "stock should reflect overridden array element: 1+99+3=103"
        );
    }
}
