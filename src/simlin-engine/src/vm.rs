// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

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

static EMPTY_OVERRIDES: LazyLock<HashMap<usize, f64>> = LazyLock::new(HashMap::new);

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

#[inline(always)]
fn eval_op2(op: Op2, l: f64, r: f64) -> f64 {
    match op {
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
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<ModuleKey, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: ModuleKey,
    pub(crate) offsets: HashMap<Ident<Canonical>, usize>,
    cached_initial_offsets: HashSet<usize>,
}

impl CompiledSimulation {
    pub(crate) fn new(
        modules: HashMap<ModuleKey, CompiledModule>,
        specs: Specs,
        root: ModuleKey,
        offsets: HashMap<Ident<Canonical>, usize>,
    ) -> Self {
        let cached_initial_offsets = collect_initial_offsets(&modules, &root, 0);
        CompiledSimulation {
            modules,
            specs,
            root,
            offsets,
            cached_initial_offsets,
        }
    }

    pub fn get_offset(&self, ident: &Ident<Canonical>) -> Option<usize> {
        self.offsets.get(ident).copied()
    }

    pub fn n_slots(&self) -> usize {
        self.modules[&self.root].n_slots
    }

    pub fn initial_offsets(&self) -> &HashSet<usize> {
        &self.cached_initial_offsets
    }
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
    // Reusable stacks (allocated once, cleared before each top-level call)
    stack: Stack,
    view_stack: Vec<RuntimeView>,
    iter_stack: Vec<IterState>,
    broadcast_stack: Vec<BroadcastState>,
    // Override values: offset -> value. Applied after each variable's
    // initial bytecode executes (evaluate-then-patch).
    overrides: HashMap<usize, f64>,
    // All absolute data-buffer offsets that are written during initials
    // (precomputed from the module tree for fast override validation).
    initial_offsets: HashSet<usize>,
}

use crate::bytecode::STACK_CAPACITY;

#[derive(Clone)]
struct Stack {
    data: [f64; STACK_CAPACITY],
    top: usize,
}

#[cfg(feature = "debug-derive")]
impl std::fmt::Debug for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("top", &self.top)
            .field("data", &&self.data[..self.top])
            .finish()
    }
}

#[allow(unsafe_code)]
impl Stack {
    fn new() -> Self {
        Stack {
            data: [0.0; STACK_CAPACITY],
            top: 0,
        }
    }
    #[inline(always)]
    fn push(&mut self, value: f64) {
        debug_assert!(self.top < STACK_CAPACITY, "stack overflow");
        // SAFETY: ByteCodeBuilder::finish() statically validates that the max
        // stack depth of all compiled bytecode is < STACK_CAPACITY, so this
        // bound cannot be exceeded at runtime. The debug_assert serves as a
        // belt-and-suspenders check during development.
        unsafe {
            *self.data.get_unchecked_mut(self.top) = value;
        }
        self.top += 1;
    }
    #[inline(always)]
    fn pop(&mut self) -> f64 {
        debug_assert!(self.top > 0, "stack underflow");
        self.top -= 1;
        // SAFETY: top was > 0 before decrement (debug_assert above), so top is now
        // in [0, STACK_CAPACITY - 1], which is a valid index into data.
        unsafe { *self.data.get_unchecked(self.top) }
    }
    #[inline(always)]
    fn len(&self) -> usize {
        self.top
    }
    #[inline(always)]
    fn clear(&mut self) {
        self.top = 0;
    }
}

/// Mutable evaluation state grouped into a single struct to reduce argument
/// count in eval functions (was 11-14 args, now 6-10).  In `eval_bytecode`,
/// the fields are destructured into local reborrows for ergonomic access;
/// for recursive `EvalModule` calls they must be re-packed into a temporary
/// `EvalState` because the borrow checker cannot split the struct across the
/// call boundary.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct EvalState<'a> {
    stack: &'a mut Stack,
    temp_storage: &'a mut [f64],
    view_stack: &'a mut Vec<RuntimeView>,
    iter_stack: &'a mut Vec<IterState>,
    broadcast_stack: &'a mut Vec<BroadcastState>,
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

/// Recursively collect all absolute initial offsets from a module and its submodules.
fn collect_initial_offsets(
    modules: &HashMap<ModuleKey, CompiledModule>,
    module_key: &ModuleKey,
    base_off: usize,
) -> HashSet<usize> {
    let mut result = HashSet::new();
    let module = &modules[module_key];

    for ci in module.compiled_initials.iter() {
        for &off in &ci.offsets {
            result.insert(base_off + off);
        }
    }

    for module_decl in &module.context.modules {
        let child_key = make_module_key(&module_decl.model_name, &module_decl.input_set);
        let child_base = base_off + module_decl.off;
        result.extend(collect_initial_offsets(modules, &child_key, child_base));
    }

    result
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
            stack: Stack::new(),
            view_stack: Vec::with_capacity(4),
            iter_stack: Vec::with_capacity(2),
            broadcast_stack: Vec::with_capacity(1),
            overrides: HashMap::new(),
            initial_offsets: sim.cached_initial_offsets,
        })
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

        self.stack.clear();
        let module_inputs: &[f64] = &[0.0; 0];
        let mut data = self.data.take().unwrap();

        let module_flows = &self.sliced_sim.flow_modules[&self.root];
        let module_stocks = &self.sliced_sim.stock_modules[&self.root];

        self.view_stack.clear();
        self.iter_stack.clear();
        self.broadcast_stack.clear();

        let mut state = EvalState {
            stack: &mut self.stack,
            temp_storage: &mut self.temp_storage,
            view_stack: &mut self.view_stack,
            iter_stack: &mut self.iter_stack,
            broadcast_stack: &mut self.broadcast_stack,
        };

        loop {
            let (curr, next) = borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
            if curr[TIME_OFF] > end {
                break;
            }

            Self::eval(
                &self.sliced_sim,
                &mut state,
                module_flows,
                0,
                module_inputs,
                curr,
                next,
            );
            Self::eval(
                &self.sliced_sim,
                &mut state,
                module_stocks,
                0,
                module_inputs,
                curr,
                next,
            );
            // Only TIME changes per step; DT, INITIAL_TIME, FINAL_TIME are
            // invariant and already set in every chunk slot during initials.
            next[TIME_OFF] = curr[TIME_OFF] + dt;

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
        let data = self.data.as_mut().unwrap();
        data[start + off] = val;
    }

    /// Read the current value of a variable by its data buffer offset.
    ///
    /// Precondition: `run_initials()` must have been called since the last
    /// `reset()`. After `reset()` but before `run_initials()`, the data buffer
    /// may contain stale values from the previous simulation run.
    pub fn get_value_now(&self, off: usize) -> f64 {
        debug_assert!(
            self.did_initials,
            "get_value_now called before run_initials; data buffer may contain stale values"
        );
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
    ///
    /// The data buffer is NOT zeroed here because `run_initials()` fully
    /// reinitializes all variable slots and pre-fills DT/INITIAL_TIME/FINAL_TIME
    /// across all chunk slots. The `did_initials` flag (reset to false here)
    /// prevents `run_to()` from executing on stale data -- it returns early
    /// if `run_initials()` has not been called since the last reset.
    pub fn reset(&mut self) {
        self.curr_chunk = 0;
        self.next_chunk = 1;
        self.did_initials = false;
        self.step_accum = 0;
        self.temp_storage.fill(0.0);
        self.stack.clear();
        self.view_stack.clear();
        self.iter_stack.clear();
        self.broadcast_stack.clear();
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
                BadOverride,
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
                BadOverride,
                format!("offset {} out of bounds (n_slots={})", off, self.n_slots)
            );
        }
        if !self.is_initial_offset(off) {
            return sim_err!(
                BadOverride,
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

        self.stack.clear();
        let module_inputs: &[f64] = &[0.0; 0];
        let mut data = self.data.take().unwrap();

        let (curr, next) = borrow_two(&mut data, self.n_slots, self.curr_chunk, self.next_chunk);
        curr[TIME_OFF] = spec_start;
        curr[DT_OFF] = dt;
        curr[INITIAL_TIME_OFF] = spec_start;
        curr[FINAL_TIME_OFF] = spec_stop;

        self.view_stack.clear();
        self.iter_stack.clear();
        self.broadcast_stack.clear();

        let mut state = EvalState {
            stack: &mut self.stack,
            temp_storage: &mut self.temp_storage,
            view_stack: &mut self.view_stack,
            iter_stack: &mut self.iter_stack,
            broadcast_stack: &mut self.broadcast_stack,
        };

        Self::eval_initials_with_overrides(
            &self.sliced_sim,
            &mut state,
            &self.root,
            0,
            module_inputs,
            curr,
            next,
            &self.overrides,
        );

        // Pre-fill DT, INITIAL_TIME, and FINAL_TIME across all chunk slots so
        // run_to only needs to advance TIME per step.
        let n_slots = self.n_slots;
        let total_chunks = self.n_chunks + 2;
        for chunk in 0..total_chunks {
            let base = chunk * n_slots;
            data[base + DT_OFF] = dt;
            data[base + INITIAL_TIME_OFF] = spec_start;
            data[base + FINAL_TIME_OFF] = spec_stop;
        }

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
        state: &mut EvalState<'_>,
        parent_context: &ByteCodeContext,
        parent_module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        id: ModuleId,
        overrides: &HashMap<usize, f64>,
    ) {
        let new_module_decl = &parent_context.modules[id as usize];
        let module_key = make_module_key(&new_module_decl.model_name, &new_module_decl.input_set);
        let module_off = parent_module_off + new_module_decl.off;

        Self::eval_initials_with_overrides(
            sliced_sim,
            state,
            &module_key,
            module_off,
            module_inputs,
            curr,
            next,
            overrides,
        );
    }

    /// Run all per-variable initials for a module (in dependency order),
    /// applying overrides after each variable's bytecode completes.
    #[allow(clippy::too_many_arguments)]
    fn eval_initials_with_overrides(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module_key: &ModuleKey,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        overrides: &HashMap<usize, f64>,
    ) {
        let module_initials = &sliced_sim.initial_modules[module_key];
        let context = &module_initials.context;
        for compiled_initial in module_initials.initials.iter() {
            Self::eval_single_initial(
                sliced_sim,
                state,
                context,
                &compiled_initial.bytecode,
                module_off,
                module_inputs,
                curr,
                next,
                overrides,
            );
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
        state: &mut EvalState<'_>,
        context: &ByteCodeContext,
        bytecode: &ByteCode,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        overrides: &HashMap<usize, f64>,
    ) {
        Self::eval_bytecode(
            sliced_sim,
            state,
            context,
            bytecode,
            StepPart::Initials,
            module_off,
            module_inputs,
            curr,
            next,
            overrides,
        );
    }

    #[inline(always)]
    fn eval(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module: &CompiledModuleSlice,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        Self::eval_bytecode(
            sliced_sim,
            state,
            &module.context,
            &module.bytecode,
            module.part,
            module_off,
            module_inputs,
            curr,
            next,
            &EMPTY_OVERRIDES,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn eval_bytecode(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        context: &ByteCodeContext,
        bytecode: &ByteCode,
        part: StepPart,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        overrides: &HashMap<usize, f64>,
    ) {
        // Destructure EvalState into local reborrows so the opcode loop can use
        // them directly.  For recursive EvalModule calls we must re-pack into a
        // temporary EvalState (and destructure again afterward) because holding
        // individual &mut borrows from the struct would prevent passing &mut EvalState.
        let mut stack = &mut *state.stack;
        let mut temp_storage = &mut *state.temp_storage;
        let mut view_stack = &mut *state.view_stack;
        let mut iter_stack = &mut *state.iter_stack;
        let mut broadcast_stack = &mut *state.broadcast_stack;

        let mut condition = false;
        let mut subscript_index: SmallVec<[(u16, u16); 4]> = SmallVec::new();
        let mut subscript_index_valid = true;

        let code = &bytecode.code;

        // PC-based loop for jump support
        let mut pc: usize = 0;
        while pc < code.len() {
            match &code[pc] {
                Opcode::Op2 { op } => {
                    let r = stack.pop();
                    let l = stack.pop();
                    stack.push(eval_op2(*op, l, r));
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
                    let mut child_state = EvalState {
                        stack,
                        temp_storage,
                        view_stack,
                        iter_stack,
                        broadcast_stack,
                    };
                    match part {
                        StepPart::Initials => {
                            Self::eval_module_initials_with_overrides(
                                sliced_sim,
                                &mut child_state,
                                context,
                                module_off,
                                &module_inputs,
                                curr,
                                next,
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
                                &mut child_state,
                                child_module,
                                child_module_off,
                                &module_inputs,
                                curr,
                                next,
                            );
                        }
                    }
                    // Recover mutable references from child_state
                    let EvalState {
                        stack: s,
                        temp_storage: ts,
                        view_stack: vs,
                        iter_stack: is_,
                        broadcast_stack: bs,
                    } = child_state;
                    stack = s;
                    temp_storage = ts;
                    view_stack = vs;
                    iter_stack = is_;
                    broadcast_stack = bs;
                }
                Opcode::AssignCurr { off } => {
                    curr[module_off + *off as usize] = stack.pop();
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignNext { off } => {
                    next[module_off + *off as usize] = stack.pop();
                    debug_assert_eq!(0, stack.len());
                }
                // === SUPERINSTRUCTIONS ===
                Opcode::AssignConstCurr { off, literal_id } => {
                    curr[module_off + *off as usize] = bytecode.literals[*literal_id as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::BinOpAssignCurr { op, off } => {
                    let r = stack.pop();
                    let l = stack.pop();
                    curr[module_off + *off as usize] = eval_op2(*op, l, r);
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::BinOpAssignNext { op, off } => {
                    let r = stack.pop();
                    let l = stack.pop();
                    next[module_off + *off as usize] = eval_op2(*op, l, r);
                    debug_assert_eq!(0, stack.len());
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
                    dim_list_id,
                } => {
                    let (n_dims, dim_ids) = context.get_dim_list(*dim_list_id);
                    let n = n_dims as usize;
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
                    dim_list_id,
                } => {
                    let (n_dims, dim_ids) = context.get_dim_list(*dim_list_id);
                    let n = n_dims as usize;
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
                    dim_list_id,
                } => {
                    let (n_dims, dims) = context.get_dim_list(*dim_list_id);
                    let n = n_dims as usize;
                    let dims_vec: SmallVec<[u16; 4]> = dims[..n].iter().copied().collect();
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
mod eval_op2_tests {
    use super::*;

    #[test]
    fn test_eval_op2_arithmetic() {
        assert_eq!(eval_op2(Op2::Add, 3.0, 4.0), 7.0);
        assert_eq!(eval_op2(Op2::Sub, 10.0, 3.0), 7.0);
        assert_eq!(eval_op2(Op2::Mul, 3.0, 4.0), 12.0);
        assert_eq!(eval_op2(Op2::Div, 10.0, 4.0), 2.5);
        assert_eq!(eval_op2(Op2::Exp, 2.0, 3.0), 8.0);
        assert_eq!(eval_op2(Op2::Mod, 7.0, 3.0), 1.0);
    }

    #[test]
    fn test_eval_op2_comparisons() {
        assert_eq!(eval_op2(Op2::Gt, 5.0, 3.0), 1.0);
        assert_eq!(eval_op2(Op2::Gt, 3.0, 5.0), 0.0);
        assert_eq!(eval_op2(Op2::Gte, 5.0, 5.0), 1.0);
        assert_eq!(eval_op2(Op2::Lt, 3.0, 5.0), 1.0);
        assert_eq!(eval_op2(Op2::Lte, 5.0, 5.0), 1.0);
        assert_eq!(eval_op2(Op2::Eq, 5.0, 5.0), 1.0);
        assert_eq!(eval_op2(Op2::Eq, 5.0, 5.1), 0.0);
    }

    #[test]
    fn test_eval_op2_logical() {
        assert_eq!(eval_op2(Op2::And, 1.0, 1.0), 1.0);
        assert_eq!(eval_op2(Op2::And, 1.0, 0.0), 0.0);
        assert_eq!(eval_op2(Op2::Or, 0.0, 1.0), 1.0);
        assert_eq!(eval_op2(Op2::Or, 0.0, 0.0), 0.0);
    }
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
        let err = vm.set_override_by_offset(99999, 1.0).unwrap_err();
        assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
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

        let err = vm
            .set_override(&canonicalize("birth_rate"), 0.5)
            .unwrap_err();
        assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
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

#[cfg(test)]
mod stack_tests {
    use super::*;

    #[test]
    fn test_push_pop_basic() {
        let mut s = Stack::new();
        s.push(1.0);
        s.push(2.0);
        s.push(3.0);
        assert_eq!(3.0, s.pop());
        assert_eq!(2.0, s.pop());
        assert_eq!(1.0, s.pop());
    }

    #[test]
    fn test_lifo_ordering() {
        let mut s = Stack::new();
        for i in 0..10 {
            s.push(i as f64);
        }
        for i in (0..10).rev() {
            assert_eq!(i as f64, s.pop());
        }
    }

    #[test]
    fn test_clear_resets_stack() {
        let mut s = Stack::new();
        s.push(1.0);
        s.push(2.0);
        assert_eq!(2, s.len());
        s.clear();
        assert_eq!(0, s.len());
    }

    #[test]
    fn test_len_tracks_size() {
        let mut s = Stack::new();
        assert_eq!(0, s.len());
        s.push(10.0);
        assert_eq!(1, s.len());
        s.push(20.0);
        assert_eq!(2, s.len());
        s.pop();
        assert_eq!(1, s.len());
        s.pop();
        assert_eq!(0, s.len());
    }

    #[test]
    fn test_full_capacity() {
        let mut s = Stack::new();
        for i in 0..STACK_CAPACITY {
            s.push(i as f64);
        }
        assert_eq!(STACK_CAPACITY, s.len());
        for i in (0..STACK_CAPACITY).rev() {
            assert_eq!(i as f64, s.pop());
        }
        assert_eq!(0, s.len());
    }

    #[test]
    fn test_interleaved_push_pop() {
        let mut s = Stack::new();
        s.push(1.0);
        s.push(2.0);
        assert_eq!(2.0, s.pop());
        s.push(3.0);
        s.push(4.0);
        assert_eq!(4.0, s.pop());
        assert_eq!(3.0, s.pop());
        assert_eq!(1.0, s.pop());
        assert_eq!(0, s.len());
    }

    #[test]
    fn test_push_after_clear() {
        let mut s = Stack::new();
        s.push(1.0);
        s.push(2.0);
        s.clear();
        s.push(42.0);
        assert_eq!(1, s.len());
        assert_eq!(42.0, s.pop());
    }

    #[test]
    fn test_negative_and_special_values() {
        let mut s = Stack::new();
        s.push(-1.0);
        s.push(0.0);
        s.push(f64::INFINITY);
        s.push(f64::NEG_INFINITY);
        s.push(f64::NAN);
        assert!(s.pop().is_nan());
        assert_eq!(f64::NEG_INFINITY, s.pop());
        assert_eq!(f64::INFINITY, s.pop());
        assert_eq!(0.0, s.pop());
        assert_eq!(-1.0, s.pop());
    }
}

#[cfg(test)]
mod superinstruction_tests {
    use super::*;
    use crate::bytecode::Opcode;
    use crate::test_common::TestProject;

    fn build_vm(tp: &TestProject) -> Vm {
        let sim = tp.build_sim().unwrap();
        let compiled = sim.compile().unwrap();
        Vm::new(compiled).unwrap()
    }

    /// Helper: collect all opcodes from the flow bytecode of the root module.
    fn flow_opcodes(vm: &Vm) -> Vec<&Opcode> {
        let bc = &vm.sliced_sim.flow_modules[&vm.root].bytecode;
        bc.code.iter().collect()
    }

    /// Helper: collect all opcodes from the stock bytecode of the root module.
    fn stock_opcodes(vm: &Vm) -> Vec<&Opcode> {
        let bc = &vm.sliced_sim.stock_modules[&vm.root].bytecode;
        bc.code.iter().collect()
    }

    // -----------------------------------------------------------------------
    // AssignConstCurr: a constant aux like `birth_rate = 0.1`
    // -----------------------------------------------------------------------

    #[test]
    fn test_assign_const_curr_present_in_bytecode() {
        let tp = TestProject::new("const_model")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "pop * rate", None)
            .stock("pop", "100", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = flow_opcodes(&vm);
        let has_assign_const = ops
            .iter()
            .any(|op| matches!(op, Opcode::AssignConstCurr { .. }));
        assert!(
            has_assign_const,
            "constant aux should produce AssignConstCurr in flow bytecode"
        );
    }

    #[test]
    fn test_assign_const_curr_simulation_result() {
        let tp = TestProject::new("const_sim")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "pop * rate", None)
            .stock("pop", "100", &["inflow"], &[], None);

        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();

        let vm_rate = &vm_results["rate"];
        let interp_rate = &interp_results["rate"];
        for (i, (v, e)) in vm_rate.iter().zip(interp_rate.iter()).enumerate() {
            assert!(
                (v - e).abs() < 1e-10,
                "rate mismatch at step {i}: vm={v}, interp={e}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // BinOpAssignCurr: e.g. `births = population * birth_rate`
    // -----------------------------------------------------------------------

    #[test]
    fn test_binop_assign_curr_present_in_bytecode() {
        let tp = TestProject::new("binop_model")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("rate", "0.1", None)
            .aux("result", "rate * 2", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = flow_opcodes(&vm);
        let has_binop_curr = ops
            .iter()
            .any(|op| matches!(op, Opcode::BinOpAssignCurr { .. }));
        assert!(
            has_binop_curr,
            "binary operation with assign should produce BinOpAssignCurr"
        );
    }

    #[test]
    fn test_binop_assign_curr_simulation_mul() {
        let tp = TestProject::new("binop_mul")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "3", None)
            .aux("b", "4", None)
            .aux("result", "a * b", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);

        let vm_results = tp.run_vm().unwrap();
        assert!(
            (vm_results["result"][0] - 12.0).abs() < 1e-10,
            "3 * 4 should equal 12"
        );
    }

    // -----------------------------------------------------------------------
    // BinOpAssignNext: stock integration `stock_next = stock + flow * dt`
    // -----------------------------------------------------------------------

    #[test]
    fn test_binop_assign_next_present_in_bytecode() {
        let tp = TestProject::new("stock_integ")
            .with_sim_time(0.0, 2.0, 1.0)
            .flow("inflow", "10", None)
            .stock("s", "0", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = stock_opcodes(&vm);
        let has_binop_next = ops
            .iter()
            .any(|op| matches!(op, Opcode::BinOpAssignNext { .. }));
        assert!(
            has_binop_next,
            "stock integration should produce BinOpAssignNext in stock bytecode"
        );
    }

    #[test]
    fn test_binop_assign_next_simulation_stock_integration() {
        let tp = TestProject::new("stock_integ_sim")
            .with_sim_time(0.0, 5.0, 1.0)
            .flow("inflow", "10", None)
            .stock("s", "0", &["inflow"], &[], None);

        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();

        let vm_s = &vm_results["s"];
        let interp_s = &interp_results["s"];

        for (i, (v, e)) in vm_s.iter().zip(interp_s.iter()).enumerate() {
            assert!(
                (v - e).abs() < 1e-10,
                "stock mismatch at step {i}: vm={v}, interp={e}"
            );
        }
        // s starts at 0, inflow=10, dt=1 => s at step 1 = 10, step 2 = 20, etc.
        assert!((vm_s[0] - 0.0).abs() < 1e-10, "stock initial should be 0");
        assert!(
            (vm_s[1] - 10.0).abs() < 1e-10,
            "stock at step 1 should be 10"
        );
    }

    // -----------------------------------------------------------------------
    // Op2 variants through BinOpAssignCurr
    // -----------------------------------------------------------------------

    fn run_binop_model(equation: &str) -> f64 {
        let tp = TestProject::new("binop_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "10", None)
            .aux("b", "3", None)
            .aux("result", equation, None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);

        let vm_results = tp.run_vm().unwrap();
        vm_results["result"][0]
    }

    #[test]
    fn test_op2_add() {
        let result = run_binop_model("a + b");
        assert!((result - 13.0).abs() < 1e-10, "10 + 3 = 13, got {result}");
    }

    #[test]
    fn test_op2_sub() {
        let result = run_binop_model("a - b");
        assert!((result - 7.0).abs() < 1e-10, "10 - 3 = 7, got {result}");
    }

    #[test]
    fn test_op2_mul() {
        let result = run_binop_model("a * b");
        assert!((result - 30.0).abs() < 1e-10, "10 * 3 = 30, got {result}");
    }

    #[test]
    fn test_op2_div() {
        let result = run_binop_model("a / b");
        assert!((result - 10.0 / 3.0).abs() < 1e-10, "10 / 3, got {result}");
    }

    #[test]
    fn test_op2_gt() {
        let result = run_binop_model("IF a > b THEN 1 ELSE 0");
        assert!(
            (result - 1.0).abs() < 1e-10,
            "10 > 3 should be true, got {result}"
        );
    }

    #[test]
    fn test_op2_lt() {
        let result = run_binop_model("IF a < b THEN 1 ELSE 0");
        assert!(
            (result - 0.0).abs() < 1e-10,
            "10 < 3 should be false, got {result}"
        );
    }

    #[test]
    fn test_op2_eq() {
        // a=10, b=3, so a=b should be false
        let tp = TestProject::new("eq_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "5", None)
            .aux("b", "5", None)
            .aux("result", "IF a = b THEN 1 ELSE 0", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!(
            (result - 1.0).abs() < 1e-10,
            "5 = 5 should be true, got {result}"
        );
    }

    #[test]
    fn test_op2_and() {
        let tp = TestProject::new("and_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "1", None)
            .aux("b", "1", None)
            .aux("result", "IF (a > 0) AND (b > 0) THEN 1 ELSE 0", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!(
            (result - 1.0).abs() < 1e-10,
            "1>0 AND 1>0 should be true, got {result}"
        );
    }

    #[test]
    fn test_op2_or() {
        let tp = TestProject::new("or_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "0", None)
            .aux("b", "1", None)
            .aux("result", "IF (a > 0) OR (b > 0) THEN 1 ELSE 0", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!(
            (result - 1.0).abs() < 1e-10,
            "0>0 OR 1>0 should be true, got {result}"
        );
    }

    // -----------------------------------------------------------------------
    // Superinstruction execution correctness across multiple timesteps
    // -----------------------------------------------------------------------

    #[test]
    fn test_superinstruction_population_model_matches_interpreter() {
        let tp = TestProject::new("pop_model")
            .with_sim_time(0.0, 10.0, 0.5)
            .aux("birth_rate", "0.1", None)
            .aux("death_rate", "0.05", None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population * death_rate", None)
            .stock("population", "1000", &["births"], &["deaths"], None);

        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();

        for var in &["population", "births", "deaths", "birth_rate", "death_rate"] {
            let vm_vals = &vm_results[*var];
            let interp_vals = &interp_results[*var];
            assert_eq!(
                vm_vals.len(),
                interp_vals.len(),
                "step count mismatch for {var}"
            );
            for (i, (v, e)) in vm_vals.iter().zip(interp_vals.iter()).enumerate() {
                assert!(
                    (v - e).abs() < 1e-10,
                    "{var} mismatch at step {i}: vm={v}, interp={e}"
                );
            }
        }
    }

    #[test]
    fn test_superinstruction_with_small_dt() {
        let tp = TestProject::new("small_dt")
            .with_sim_time(0.0, 1.0, 0.125)
            .aux("rate", "0.5", None)
            .flow("growth", "s * rate", None)
            .stock("s", "10", &["growth"], &[], None);

        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();

        let vm_s = &vm_results["s"];
        let interp_s = &interp_results["s"];
        for (i, (v, e)) in vm_s.iter().zip(interp_s.iter()).enumerate() {
            assert!(
                (v - e).abs() < 1e-10,
                "s mismatch at step {i}: vm={v}, interp={e}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Op2 variants through *fused* BinOpAssignCurr superinstruction.
    // The run_binop_model tests above use IF/THEN/ELSE which goes through
    // SetCond+If, not the fused path. These tests use direct assignment
    // to ensure the BinOpAssignCurr handler is exercised for each Op2.
    // -----------------------------------------------------------------------

    fn run_fused_binop(equation: &str) -> f64 {
        // equation should be a direct binary op like "a ^ b" assigned to result
        let tp = TestProject::new("fused_binop")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "10", None)
            .aux("b", "3", None)
            .aux("result", equation, None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        vm_results["result"][0]
    }

    #[test]
    fn test_fused_binop_exp() {
        let result = run_fused_binop("a ^ b");
        assert!((result - 1000.0).abs() < 1e-10, "10^3 = 1000, got {result}");
    }

    #[test]
    fn test_fused_binop_div() {
        let result = run_fused_binop("a / b");
        assert!((result - 10.0 / 3.0).abs() < 1e-10, "10/3, got {result}");
    }

    #[test]
    fn test_fused_binop_mod() {
        let result = run_fused_binop("a MOD b");
        assert!((result - 1.0).abs() < 1e-10, "10 mod 3 = 1, got {result}");
    }

    #[test]
    fn test_fused_binop_gt() {
        let result = run_fused_binop("a > b");
        assert!((result - 1.0).abs() < 1e-10, "10 > 3 = 1, got {result}");
    }

    #[test]
    fn test_fused_binop_gte() {
        let result = run_fused_binop("a >= b");
        assert!((result - 1.0).abs() < 1e-10, "10 >= 3 = 1, got {result}");
    }

    #[test]
    fn test_fused_binop_lt() {
        let result = run_fused_binop("a < b");
        assert!((result - 0.0).abs() < 1e-10, "10 < 3 = 0, got {result}");
    }

    #[test]
    fn test_fused_binop_lte() {
        let result = run_fused_binop("a <= b");
        assert!((result - 0.0).abs() < 1e-10, "10 <= 3 = 0, got {result}");
    }

    #[test]
    fn test_fused_binop_eq() {
        // Use equal values so we test the true case
        let tp = TestProject::new("fused_eq")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "5", None)
            .aux("b", "5", None)
            .aux("result", "a = b", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 1.0).abs() < 1e-10, "5 = 5 = 1, got {result}");
    }

    #[test]
    fn test_fused_binop_and() {
        let tp = TestProject::new("fused_and")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "1", None)
            .aux("b", "1", None)
            .aux("result", "a AND b", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 1.0).abs() < 1e-10, "1 AND 1 = 1, got {result}");
    }

    #[test]
    fn test_fused_binop_or() {
        let tp = TestProject::new("fused_or")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "0", None)
            .aux("b", "1", None)
            .aux("result", "a OR b", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 1.0).abs() < 1e-10, "0 OR 1 = 1, got {result}");
    }

    // -----------------------------------------------------------------------
    // Op2 variants through fused BinOpAssignNext (stock integration)
    // -----------------------------------------------------------------------

    #[test]
    fn test_fused_binop_next_sub() {
        // stock with only outflow exercises Sub in AssignNext
        let tp = TestProject::new("fused_next_sub")
            .with_sim_time(0.0, 3.0, 1.0)
            .flow("outflow", "5", None)
            .stock("s", "100", &[], &["outflow"], None);
        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();
        let vm_s = &vm_results["s"];
        let interp_s = &interp_results["s"];
        for (i, (v, e)) in vm_s.iter().zip(interp_s.iter()).enumerate() {
            assert!(
                (v - e).abs() < 1e-10,
                "s mismatch at step {i}: vm={v}, interp={e}"
            );
        }
        assert!((vm_s[0] - 100.0).abs() < 1e-10, "initial should be 100");
        assert!(
            (vm_s[1] - 95.0).abs() < 1e-10,
            "step 1 should be 95 (100 - 5)"
        );
    }

    // -----------------------------------------------------------------------
    // Unfused Op2 path: operations consumed by further stack ops
    // -----------------------------------------------------------------------

    #[test]
    fn test_unfused_op2_exp_in_expression() {
        // a^b + 1: the ^ result feeds into +, so Op2::Exp can't be fused with Assign
        let tp = TestProject::new("unfused_exp")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "2", None)
            .aux("b", "3", None)
            .aux("result", "a ^ b + 1", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 9.0).abs() < 1e-10, "2^3 + 1 = 9, got {result}");
    }

    #[test]
    fn test_unfused_op2_div_in_expression() {
        let tp = TestProject::new("unfused_div")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "10", None)
            .aux("b", "4", None)
            .aux("result", "a / b + 1", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 3.5).abs() < 1e-10, "10/4 + 1 = 3.5, got {result}");
    }

    #[test]
    fn test_unfused_op2_mod_in_expression() {
        let tp = TestProject::new("unfused_mod")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "10", None)
            .aux("b", "3", None)
            .aux("result", "a MOD b + 1", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!(
            (result - 2.0).abs() < 1e-10,
            "10 mod 3 + 1 = 2, got {result}"
        );
    }

    #[test]
    fn test_unfused_not_operator() {
        let tp = TestProject::new("unfused_not")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "0", None)
            .aux("result", "NOT a", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["result"][0];
        assert!((result - 1.0).abs() < 1e-10, "NOT 0 = 1, got {result}");
    }

    #[test]
    fn test_unfused_comparison_gte_lte_in_expression() {
        // Use >= and <= as intermediate values consumed by further ops
        let tp = TestProject::new("unfused_cmp")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "5", None)
            .aux("b", "5", None)
            .aux("gte_result", "(a >= b) + (a <= b)", None)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);
        let vm_results = tp.run_vm().unwrap();
        let result = vm_results["gte_result"][0];
        assert!(
            (result - 2.0).abs() < 1e-10,
            "(5>=5) + (5<=5) = 1+1 = 2, got {result}"
        );
    }

    #[test]
    fn test_multiple_superinstructions_in_one_model() {
        let tp = TestProject::new("multi_super")
            .with_sim_time(0.0, 3.0, 1.0)
            .aux("const_a", "2", None)
            .aux("const_b", "3", None)
            .aux("product", "const_a * const_b", None)
            .aux("sum", "const_a + const_b", None)
            .flow("inflow", "product + sum", None)
            .stock("s", "0", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = flow_opcodes(&vm);

        // There should be at least 2 AssignConstCurr (for const_a, const_b)
        let const_count = ops
            .iter()
            .filter(|op| matches!(op, Opcode::AssignConstCurr { .. }))
            .count();
        assert!(
            const_count >= 2,
            "expected at least 2 AssignConstCurr, got {const_count}"
        );

        let vm_results = tp.run_vm().unwrap();
        let interp_results = tp.run_interpreter().unwrap();

        // product = 2*3 = 6, sum = 2+3 = 5, inflow = 11
        // s starts at 0, gains 11 per step
        let vm_s = &vm_results["s"];
        let interp_s = &interp_results["s"];
        for (i, (v, e)) in vm_s.iter().zip(interp_s.iter()).enumerate() {
            assert!(
                (v - e).abs() < 1e-10,
                "s mismatch at step {i}: vm={v}, interp={e}"
            );
        }
        assert!(
            (vm_s[1] - 11.0).abs() < 1e-10,
            "s at step 1 should be 11, got {}",
            vm_s[1]
        );
    }
}

#[cfg(test)]
mod vm_reset_run_to_and_constants_tests {
    use super::*;
    use crate::canonicalize;
    use crate::datamodel;
    use crate::test_common::TestProject;

    fn pop_model() -> TestProject {
        TestProject::new("pop_model")
            .with_sim_time(0.0, 100.0, 1.0)
            .aux("birth_rate", "0.1", None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population / 80", None)
            .stock("population", "100", &["births"], &["deaths"], None)
    }

    fn build_compiled(tp: &TestProject) -> CompiledSimulation {
        let sim = tp.build_sim().unwrap();
        sim.compile().unwrap()
    }

    // ================================================================
    // Multiple reset cycles
    // ================================================================

    #[test]
    fn test_multiple_reset_cycles_produce_identical_results() {
        let compiled = build_compiled(&pop_model());
        let mut vm = Vm::new(compiled).unwrap();

        vm.run_to_end().unwrap();
        let ref_series = vm.get_series(&canonicalize("population")).unwrap();

        for cycle in 1..=5 {
            vm.reset();
            vm.run_to_end().unwrap();
            let series = vm.get_series(&canonicalize("population")).unwrap();
            assert_eq!(
                series.len(),
                ref_series.len(),
                "cycle {cycle}: series length should match"
            );
            for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-10,
                    "cycle {cycle}, step {step}: {a} vs {b}"
                );
            }
        }
    }

    // ================================================================
    // Reset after partial run with different dt values
    // ================================================================

    #[test]
    fn test_reset_after_partial_run_dt_quarter() {
        let tp = TestProject::new("dt_quarter")
            .with_sim_time(0.0, 10.0, 0.25)
            .aux("rate", "0.05", None)
            .flow("inflow", "stock * rate", None)
            .stock("stock", "100", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);

        let mut vm_ref = Vm::new(compiled.clone()).unwrap();
        vm_ref.run_to_end().unwrap();
        let ref_series = vm_ref.get_series(&canonicalize("stock")).unwrap();

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to(5.0).unwrap();
        vm.reset();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("stock")).unwrap();

        assert_eq!(series.len(), ref_series.len());
        for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "step {step}: reference {a} vs reset {b}"
            );
        }
    }

    #[test]
    fn test_reset_after_partial_run_dt_half() {
        let tp = TestProject::new("dt_half")
            .with_sim_time(0.0, 20.0, 0.5)
            .aux("rate", "0.03", None)
            .flow("inflow", "stock * rate", None)
            .stock("stock", "50", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);

        let mut vm_ref = Vm::new(compiled.clone()).unwrap();
        vm_ref.run_to_end().unwrap();
        let ref_series = vm_ref.get_series(&canonicalize("stock")).unwrap();

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to(10.0).unwrap();
        vm.reset();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("stock")).unwrap();

        assert_eq!(series.len(), ref_series.len());
        for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "step {step}: reference {a} vs reset {b}"
            );
        }
    }

    // ================================================================
    // Pre-filled constants verification
    // ================================================================

    #[test]
    fn test_prefilled_constants_after_run_initials() {
        let tp = TestProject::new("constants_check")
            .with_sim_time(5.0, 50.0, 0.5)
            .flow("inflow", "0", None)
            .stock("s", "10", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();

        assert_eq!(vm.get_value_now(TIME_OFF), 5.0);
        assert_eq!(vm.get_value_now(DT_OFF), 0.5);
        assert_eq!(vm.get_value_now(INITIAL_TIME_OFF), 5.0);
        assert_eq!(vm.get_value_now(FINAL_TIME_OFF), 50.0);

        // DT/INITIAL_TIME/FINAL_TIME are pre-filled in every chunk slot during initials
        let data = vm.data.as_ref().unwrap();
        let n_slots = vm.n_slots;
        let total_chunks = vm.n_chunks + 2;
        for chunk in 1..total_chunks {
            let base = chunk * n_slots;
            assert_eq!(data[base + DT_OFF], 0.5, "DT in chunk {chunk}");
            assert_eq!(
                data[base + INITIAL_TIME_OFF],
                5.0,
                "INITIAL_TIME in chunk {chunk}"
            );
            assert_eq!(
                data[base + FINAL_TIME_OFF],
                50.0,
                "FINAL_TIME in chunk {chunk}"
            );
        }
    }

    #[test]
    fn test_constants_remain_correct_throughout_simulation() {
        let tp = TestProject::new("constants_during_sim")
            .with_sim_time(0.0, 10.0, 1.0)
            .flow("inflow", "1", None)
            .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        let data = vm.data.as_ref().unwrap();
        let n_slots = vm.n_slots;
        for chunk in 0..vm.n_chunks {
            let base = chunk * n_slots;
            assert_eq!(data[base + DT_OFF], 1.0, "DT in chunk {chunk}");
            assert_eq!(
                data[base + INITIAL_TIME_OFF],
                0.0,
                "INITIAL_TIME in chunk {chunk}"
            );
            assert_eq!(
                data[base + FINAL_TIME_OFF],
                10.0,
                "FINAL_TIME in chunk {chunk}"
            );
        }
    }

    // ================================================================
    // TIME series correctness
    // ================================================================

    #[test]
    fn test_time_advances_by_dt_each_step() {
        let tp = TestProject::new("time_series")
            .with_sim_time(0.0, 5.0, 1.0)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        let data = vm.data.as_ref().unwrap();
        let n_slots = vm.n_slots;
        for chunk in 0..vm.n_chunks {
            let base = chunk * n_slots;
            let expected_time = chunk as f64;
            assert!(
                (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
                "chunk {chunk}: TIME={}, expected {}",
                data[base + TIME_OFF],
                expected_time
            );
        }
    }

    #[test]
    fn test_time_series_with_fractional_dt() {
        // Use save_step=dt so every step is saved
        let tp = TestProject::new_with_specs(
            "time_frac",
            datamodel::SimSpecs {
                start: 0.0,
                stop: 2.0,
                dt: datamodel::Dt::Dt(0.25),
                save_step: Some(datamodel::Dt::Dt(0.25)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        )
        .flow("inflow", "0", None)
        .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        let data = vm.data.as_ref().unwrap();
        let n_slots = vm.n_slots;
        // Expected: 0.0, 0.25, 0.5, ..., 2.0 => 9 saved steps
        let expected_steps = 9;
        assert_eq!(vm.n_chunks, expected_steps);
        for chunk in 0..vm.n_chunks {
            let base = chunk * n_slots;
            let expected_time = chunk as f64 * 0.25;
            assert!(
                (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
                "chunk {chunk}: TIME={}, expected {}",
                data[base + TIME_OFF],
                expected_time
            );
        }
    }

    #[test]
    fn test_time_series_with_nonzero_start() {
        let tp = TestProject::new("time_nonzero")
            .with_sim_time(10.0, 15.0, 1.0)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        let data = vm.data.as_ref().unwrap();
        let n_slots = vm.n_slots;
        for chunk in 0..vm.n_chunks {
            let base = chunk * n_slots;
            let expected_time = 10.0 + chunk as f64;
            assert!(
                (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
                "chunk {chunk}: TIME={}, expected {}",
                data[base + TIME_OFF],
                expected_time
            );
        }
    }

    // ================================================================
    // set_value_now / get_value_now
    // ================================================================

    #[test]
    fn test_set_and_get_value_now() {
        let tp = TestProject::new("set_get")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "stock * rate", None)
            .stock("stock", "100", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();

        let stock_off = vm.get_offset(&canonicalize("stock")).unwrap();

        assert_eq!(vm.get_value_now(stock_off), 100.0);

        vm.set_value_now(stock_off, 42.0);
        assert_eq!(vm.get_value_now(stock_off), 42.0);

        vm.set_value_now(stock_off, -7.5);
        assert_eq!(vm.get_value_now(stock_off), -7.5);
    }

    #[test]
    fn test_set_value_now_for_special_offsets() {
        let tp = TestProject::new("set_specials")
            .with_sim_time(0.0, 10.0, 1.0)
            .flow("inflow", "0", None)
            .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_initials().unwrap();

        assert_eq!(vm.get_value_now(TIME_OFF), 0.0);
        assert_eq!(vm.get_value_now(DT_OFF), 1.0);
        assert_eq!(vm.get_value_now(INITIAL_TIME_OFF), 0.0);
        assert_eq!(vm.get_value_now(FINAL_TIME_OFF), 10.0);

        vm.set_value_now(TIME_OFF, 99.0);
        assert_eq!(vm.get_value_now(TIME_OFF), 99.0);
    }

    #[test]
    fn test_set_value_now_after_run_initials_affects_simulation() {
        let tp = TestProject::new("set_after_init")
            .with_sim_time(0.0, 5.0, 1.0)
            .flow("inflow", "stock * 0.1", None)
            .stock("stock", "100", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);

        let mut vm1 = Vm::new(compiled.clone()).unwrap();
        vm1.run_to_end().unwrap();
        let series1 = vm1.get_series(&canonicalize("stock")).unwrap();

        let mut vm2 = Vm::new(compiled).unwrap();
        vm2.run_initials().unwrap();
        let stock_off = vm2.get_offset(&canonicalize("stock")).unwrap();
        vm2.set_value_now(stock_off, 200.0);
        vm2.run_to_end().unwrap();
        let series2 = vm2.get_series(&canonicalize("stock")).unwrap();

        assert_eq!(series1[0], 100.0);
        assert_eq!(series2[0], 200.0);
        for step in 1..series1.len() {
            assert!(
                series2[step] > series1[step],
                "step {step}: stock with init=200 ({}) should be > stock with init=100 ({})",
                series2[step],
                series1[step]
            );
        }
    }

    // ================================================================
    // run_to with partial ranges
    // ================================================================

    #[test]
    fn test_run_to_partial_then_continue_matches_full_run() {
        let tp = pop_model();
        let compiled = build_compiled(&tp);

        let mut vm_full = Vm::new(compiled.clone()).unwrap();
        vm_full.run_to_end().unwrap();
        let full_series = vm_full.get_series(&canonicalize("population")).unwrap();

        let mut vm_partial = Vm::new(compiled).unwrap();
        vm_partial.run_to(50.0).unwrap();
        vm_partial.run_to_end().unwrap();
        let partial_series = vm_partial.get_series(&canonicalize("population")).unwrap();

        assert_eq!(full_series.len(), partial_series.len());
        for (step, (a, b)) in full_series.iter().zip(partial_series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "step {step}: full={a} vs partial+continue={b}"
            );
        }
    }

    #[test]
    fn test_run_to_multiple_segments_matches_full_run() {
        let tp = pop_model();
        let compiled = build_compiled(&tp);

        let mut vm_full = Vm::new(compiled.clone()).unwrap();
        vm_full.run_to_end().unwrap();
        let full_series = vm_full.get_series(&canonicalize("population")).unwrap();

        let mut vm_seg = Vm::new(compiled).unwrap();
        vm_seg.run_to(25.0).unwrap();
        vm_seg.run_to(50.0).unwrap();
        vm_seg.run_to(75.0).unwrap();
        vm_seg.run_to_end().unwrap();
        let seg_series = vm_seg.get_series(&canonicalize("population")).unwrap();

        assert_eq!(full_series.len(), seg_series.len());
        for (step, (a, b)) in full_series.iter().zip(seg_series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "step {step}: full={a} vs segmented={b}"
            );
        }
    }

    // ================================================================
    // Non-default save_every (save_step != dt)
    // ================================================================

    #[test]
    fn test_save_every_2_with_dt_1() {
        let tp = TestProject::new_with_specs(
            "save_every_test",
            datamodel::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: Some(datamodel::Dt::Dt(2.0)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        )
        .flow("inflow", "1", None)
        .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        // save_step=2, dt=1, start=0, stop=10: saved at t=0,2,4,6,8,10 => 6 points
        assert_eq!(series.len(), 6, "should have 6 saved points");
        let expected = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
        for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 1e-10,
                "saved point {i}: actual={actual}, expected={exp}"
            );
        }
    }

    #[test]
    fn test_save_every_with_fractional_dt() {
        let tp = TestProject::new_with_specs(
            "save_frac",
            datamodel::SimSpecs {
                start: 0.0,
                stop: 4.0,
                dt: datamodel::Dt::Dt(0.5),
                save_step: Some(datamodel::Dt::Dt(1.0)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        )
        .flow("inflow", "2", None)
        .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        // save_step=1, dt=0.5, start=0, stop=4: saved at t=0,1,2,3,4 => 5 points
        assert_eq!(series.len(), 5, "should have 5 saved points");
        // s increases by inflow*dt = 2*0.5 = 1.0 per dt step.
        // At save points: t=0: 0, t=1: 2, t=2: 4, t=3: 6, t=4: 8
        let expected = [0.0, 2.0, 4.0, 6.0, 8.0];
        for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 1e-10,
                "saved point {i}: actual={actual}, expected={exp}"
            );
        }
    }

    #[test]
    fn test_save_every_matches_dt_gives_all_steps() {
        let tp = TestProject::new("save_all")
            .with_sim_time(0.0, 5.0, 1.0)
            .flow("inflow", "1", None)
            .stock("s", "0", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        assert_eq!(series.len(), 6, "should have 6 saved points");
        let expected = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 1e-10,
                "saved point {i}: actual={actual}, expected={exp}"
            );
        }
    }

    // ================================================================
    // Reset clears temp_storage
    // ================================================================

    #[test]
    fn test_reset_zeroes_temp_storage() {
        let tp = pop_model();
        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();

        vm.reset();

        for (i, &val) in vm.temp_storage.iter().enumerate() {
            assert_eq!(val, 0.0, "temp_storage[{i}] should be 0 after reset");
        }
    }

    // ================================================================
    // Simulation produces correct numerical results
    // ================================================================

    #[test]
    fn test_exponential_growth_euler() {
        // ds/dt = s * 0.1, s(0) = 100, dt = 1
        let tp = TestProject::new("exp_growth")
            .with_sim_time(0.0, 5.0, 1.0)
            .flow("growth", "s * 0.1", None)
            .stock("s", "100", &["growth"], &[], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        // Euler: s(t+1) = s(t) * 1.1
        let expected = [100.0, 110.0, 121.0, 133.1, 146.41, 161.051];
        assert_eq!(series.len(), expected.len());
        for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - exp).abs() < 1e-6,
                "step {i}: actual={actual}, expected={exp}"
            );
        }
    }

    #[test]
    fn test_decay_model_with_small_dt() {
        // ds/dt = -s * 0.1, dt = 0.25, save_step = 0.25 so every step is saved
        let tp = TestProject::new_with_specs(
            "decay",
            datamodel::SimSpecs {
                start: 0.0,
                stop: 1.0,
                dt: datamodel::Dt::Dt(0.25),
                save_step: Some(datamodel::Dt::Dt(0.25)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        )
        .flow("decay", "s * 0.1", None)
        .stock("s", "100", &[], &["decay"], None);

        let compiled = build_compiled(&tp);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        // s(t+dt) = s(t) * (1 - 0.1*0.25) = s(t) * 0.975
        assert_eq!(series.len(), 5, "5 saved points at dt=0.25 from 0 to 1");
        let mut expected = 100.0;
        assert!((series[0] - expected).abs() < 1e-10);
        for step in 1..5 {
            expected *= 0.975;
            assert!(
                (series[step] - expected).abs() < 1e-10,
                "step {step}: actual={}, expected={}",
                series[step],
                expected
            );
        }
    }

    // ================================================================
    // Reset with save_every > 1
    // ================================================================

    #[test]
    fn test_reset_with_save_every_produces_identical_results() {
        let tp = TestProject::new_with_specs(
            "save_reset",
            datamodel::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(0.5),
                save_step: Some(datamodel::Dt::Dt(2.0)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        )
        .flow("inflow", "s * 0.1", None)
        .stock("s", "100", &["inflow"], &[], None);

        let compiled = build_compiled(&tp);

        let mut vm_ref = Vm::new(compiled.clone()).unwrap();
        vm_ref.run_to_end().unwrap();
        let ref_series = vm_ref.get_series(&canonicalize("s")).unwrap();

        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        vm.reset();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&canonicalize("s")).unwrap();

        assert_eq!(ref_series.len(), series.len());
        for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "step {step}: reference={a} vs reset={b}"
            );
        }
    }
}
