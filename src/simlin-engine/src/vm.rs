// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::alloc::allocate_available;
use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledInitial, CompiledModule, DimId, LookupMode, Op2,
    Opcode, RuntimeView, STACK_CAPACITY, TempId, ViewStorage,
};
use crate::common::{Canonical, Error, ErrorCode, ErrorKind, Ident, Result};
use crate::dimensions::match_dimensions_two_pass;
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
    let is_false = crate::float::approx_eq(n, 0.0);
    !is_false
}

/// The single authority for binary-op runtime semantics. `pub(crate)` because
/// the compiler's constant-folding pass (`compiler::fold`) calls it to compute
/// folded literals -- folding MUST be bit-identical to what this interpreter
/// would have produced, and calling the interpreter's own function guarantees
/// that by construction.
#[inline(always)]
pub(crate) fn eval_op2(op: Op2, l: f64, r: f64) -> f64 {
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
        Op2::Eq => crate::float::approx_eq(l, r) as i8 as f64,
        Op2::And => (is_truthy(l) && is_truthy(r)) as i8 as f64,
        Op2::Or => (is_truthy(l) || is_truthy(r)) as i8 as f64,
    }
}

/// Identifies a literal in a specific bytecode object that must be mutated
/// when a constant's value is overridden via `set_value`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
enum BytecodeLocation {
    /// A literal in a flows or stocks module's shared bytecode.
    FlowOrStock {
        module_key: ModuleKey,
        part: StepPart,
        literal_id: u16,
    },
    /// A literal in a specific CompiledInitial's bytecode.
    Initial {
        module_key: ModuleKey,
        initial_index: usize,
        literal_id: u16,
    },
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct CompiledSimulation {
    pub(crate) modules: HashMap<ModuleKey, CompiledModule>,
    pub(crate) specs: Specs,
    pub(crate) root: ModuleKey,
    pub(crate) offsets: HashMap<Ident<Canonical>, usize>,
    cached_constant_info: HashMap<usize, Vec<BytecodeLocation>>,
}

impl CompiledSimulation {
    pub(crate) fn new(
        modules: HashMap<ModuleKey, CompiledModule>,
        specs: Specs,
        root: ModuleKey,
        offsets: HashMap<Ident<Canonical>, usize>,
    ) -> Self {
        let cached_constant_info = collect_constant_info(&modules, &root, 0);
        CompiledSimulation {
            modules,
            specs,
            root,
            offsets,
            cached_constant_info,
        }
    }

    pub fn get_offset(&self, ident: &Ident<Canonical>) -> Option<usize> {
        self.offsets.get(ident).copied()
    }

    pub fn n_slots(&self) -> usize {
        self.modules.get(&self.root).map_or(0, |m| m.n_slots)
    }

    /// Absolute data-buffer offsets written by the root module's run-invariant
    /// flow prefix (GH #712, time-invariant hoisting). These are the slots whose
    /// value, by the classifier's verdict, must be bit-constant across every
    /// saved step. The soundness oracle test asserts exactly that. Returns an
    /// empty vector when no flow variable is run-invariant.
    ///
    /// The invariant prefix is `root.compiled_flows.code[0..flows_invariant_
    /// opcode_len]`; its `AssignCurr`/`AssignConstCurr`/`BinOpAssignCurr` target
    /// offsets are the written slots. The root module's slots are already
    /// absolute (it carries the +IMPLICIT_VAR_COUNT shift), so no base offset
    /// is added.
    ///
    /// **Safety contract**: this reads `CompiledSimulation.modules`, which holds
    /// the PRE-`fuse_three_address` bytecode (the salsa-cached artifact). This
    /// is intentional and required for correctness: `fuse_three_address` runs
    /// on the `Vm`'s private execution copy and replaces `AssignCurr`-family
    /// opcodes with fused forms (`BinVarVar`, etc.) that do NOT appear here.
    /// If this function were ever pointed at fused bytecode, the assignment
    /// scan would miss fused writes and return an incomplete offset set,
    /// silently weakening the oracle. The contract is maintained by the
    /// `Vm::new` code that builds `ResolvedModule`s by cloning `compiled_flows`
    /// out of this `CompiledSimulation` and fusing the clone separately.
    #[doc(hidden)] // test-support: used by the oracle in tests/integration/simulate.rs
    pub fn invariant_flow_offsets(&self) -> Vec<usize> {
        let Some(module) = self.modules.get(&self.root) else {
            return Vec::new();
        };
        let len = module.flows_invariant_opcode_len;
        if len == 0 {
            return Vec::new();
        }
        let prefix = &module.compiled_flows.code[..len.min(module.compiled_flows.code.len())];
        let mut offsets: Vec<usize> = prefix
            .iter()
            .filter_map(|op| match op {
                Opcode::AssignCurr { off }
                | Opcode::AssignConstCurr { off, .. }
                | Opcode::BinOpAssignCurr { off, .. } => Some(*off as usize),
                _ => None,
            })
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        offsets
    }

    /// The root module's invariant flow-prefix opcode length (GH #712). 0 when
    /// no flow variable is run-invariant. Exposed for the partition test.
    #[doc(hidden)] // test-support: used by the oracle in tests/integration/simulate.rs
    pub fn flows_invariant_opcode_len(&self) -> usize {
        self.modules
            .get(&self.root)
            .map_or(0, |m| m.flows_invariant_opcode_len)
    }

    pub fn is_constant_offset(&self, off: usize) -> bool {
        self.cached_constant_info.contains_key(&off)
    }

    /// Retract `offsets` from the overridable-constant set (GH #871).
    ///
    /// The conveyor/queue build path calls this with every pass-written slot:
    /// the expansion compiles each pass-driven flow to a placeholder
    /// `AssignConstCurr 0`, which the flows-phase classification above would
    /// otherwise treat as an overridable constant -- but the conveyor/queue
    /// pass overwrites those slots every step, so an accepted override could
    /// never affect the simulation. Retracting them makes `set_value` /
    /// `is_constant_offset` reject them with `BadOverride`, exactly like any
    /// other computed flow.
    pub(crate) fn exclude_overridable_offsets(&mut self, offsets: impl IntoIterator<Item = usize>) {
        for off in offsets {
            self.cached_constant_info.remove(&off);
        }
    }

    /// The full set of overridable constant offsets (absolute data-buffer
    /// offsets), i.e. every offset for which [`is_constant_offset`] is true.
    /// These are the offsets with an `AssignConstCurr` in some module's flows
    /// phase (see `collect_constant_info`), minus any the special conveyor/
    /// queue build path retracted via [`exclude_overridable_offsets`] (a
    /// conveyor/queue model never reaches the wasm backend, so the wasmgen
    /// parity assertion only ever sees the un-retracted set);
    /// `set_value`/`set_value_by_offset` accept exactly these. The wasm
    /// backend reads this to size and initialize its constants-override region
    /// so a blob's `set_value` accepts the same set the VM does.
    ///
    /// [`exclude_overridable_offsets`]: Self::exclude_overridable_offsets
    ///
    /// [`is_constant_offset`]: Self::is_constant_offset
    pub(crate) fn constant_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.cached_constant_info.keys().copied()
    }
}

/// One unique compiled module (a distinct `(model_name, input_set)`), holding
/// its three phase programs plus the resolved child-module indices for its
/// `EvalModule` opcodes.
///
/// `child_targets[decl_id]` is the index into `CompiledSlicedSimulation.modules`
/// of the module that `context.modules[decl_id]` instantiates. Resolving these
/// once at `Vm::new` lets the `EvalModule` opcode do a plain array index in the
/// hot loop instead of cloning a `(String, BTreeSet<String>)` key and SipHashing
/// it for a `HashMap` lookup on every module evaluation, every timestep.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct ResolvedModule {
    #[allow(dead_code)]
    ident: Ident<Canonical>,
    context: Arc<ByteCodeContext>,
    initials: Arc<Vec<CompiledInitial>>,
    flows: Arc<ByteCode>,
    stocks: Arc<ByteCode>,
    child_targets: Vec<u32>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct CompiledSlicedSimulation {
    /// All unique compiled modules, indexed by the integer ids stored in
    /// `child_targets` (and `root_idx`).
    modules: Vec<ResolvedModule>,
    root_idx: usize,
    /// `ModuleKey` -> module index. Used only by the cold `set_value` /
    /// `clear_values` literal-override paths (which still address modules by
    /// key via `BytecodeLocation`); never consulted in the hot eval loop.
    key_to_idx: HashMap<ModuleKey, u32>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    // Maps absolute offset -> all bytecode locations containing that constant's literal.
    // Used by set_value to find and mutate the right literals, and for validation.
    constant_info: HashMap<usize, Vec<BytecodeLocation>>,
    // Tracks original literal values before override, keyed by absolute offset.
    // Each entry stores the locations and their original values so clear_values can restore them.
    original_literals: HashMap<usize, Vec<(BytecodeLocation, f64)>>,
    // Snapshot of curr[] captured after the initials phase (t=0).
    // Used by LoadInitial opcode to freeze a variable's initial value.
    initial_values: Box<[f64]>,
    // Snapshot of curr[] taken after stocks but before the time advance
    // each timestep. LoadPrev reads from this buffer once prev_values_valid
    // is true; before that it returns the per-callsite fallback instead.
    prev_values: Box<[f64]>,
    // Flat list of absolute stock offsets in the data buffer.
    // Collected from stock bytecode; includes submodule stocks.
    // Empty for Euler (no RK scratch needed).
    stock_offsets: Vec<usize>,
    // RK scratch space: [saved_stocks(N) | accumulator(N)].
    // Allocated once in new(), empty for Euler.
    rk_scratch: Vec<f64>,
    // True after the first prev_values snapshot has been taken.
    // Used to set use_prev_fallback in EvalState so that LoadPrev
    // returns the fallback during the initial timestep even when
    // RK stages advance TIME away from INITIAL_TIME.
    prev_values_valid: bool,
    // Test-only: fill the `next` chunk with a sentinel at the top of every
    // Euler step. See `poison_next_chunk_for_test`. Gated with its setter so a
    // production build carries neither the flag nor the branch that reads it.
    #[cfg(any(test, feature = "test-support"))]
    poison_next: bool,
    // Conveyor support (docs/design/conveyors.md §9.3). Empty for every
    // non-conveyor model, and all conveyor logic is guarded on a non-empty
    // plan list -- so an ordinary simulation runs with zero overhead and
    // byte-identical behavior. `conveyors` is the per-belt side table (§4.2),
    // rebuilt from the initials-populated buffer on each `run_initials`.
    conveyor_plans: Vec<crate::conveyor_compile::ConveyorPlan>,
    conveyors: Vec<crate::conveyor::ConveyorState>,
    // Last integer time unit seen by the conveyor pass, for the discrete
    // per-time-unit in_limit budget reset (§6.3).
    conveyor_last_unit: i64,
    // Queue support (docs/design/queues.md §10.3). Empty for every non-queue
    // model, and all queue logic is guarded on a non-empty plan list -- so an
    // ordinary simulation runs with zero overhead and byte-identical behavior.
    // Queues and conveyors coexist: a model may carry both plan sets and both
    // side tables, and both passes run in the same between-Flows-and-Stocks slot.
    // `queues` is the per-queue FIFO side table (§4.1), rebuilt from the
    // initials-populated buffer on each `run_initials`.
    queue_plans: Vec<crate::queue_compile::QueuePlan>,
    queues: Vec<crate::queue::QueueState>,
    // The queue-conveyor coupling table (docs/design/queues.md §9). The
    // coupling is fixed at build time (apply_couplings stamps it onto the
    // plans once), so the table is derived from the two plan sets whenever
    // either is attached -- both setters rebuild it, so attachment order
    // does not matter -- rather than rebuilt every Euler step (GH #878).
    // Empty (any = false) for every uncoupled model, including a Vm whose
    // setters were never called; reset() keeps the plans, so it stays valid.
    coupling: crate::queue_compile::CouplingTable,
}

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
        // SAFETY: compiler::symbolic::resolve_bytecode() statically validates
        // that the max stack depth of all compiled bytecode is < STACK_CAPACITY,
        // so this bound cannot be exceeded at runtime. That function is the ONLY
        // place concrete bytecode is produced, so nothing can reach the VM
        // without passing the check; it reports over-depth as a compile Err
        // rather than aborting, so an unchecked program is not executed, it is
        // rejected. (The check lived in the per-fragment `ByteCodeBuilder` until
        // GH #964 made codegen emit symbolic bytecode; it did not go away with
        // the builder.) The debug_assert serves as a belt-and-suspenders check
        // during development.
        unsafe {
            *self.data.get_unchecked_mut(self.top) = value;
        }
        self.top += 1;
    }
    #[inline(always)]
    fn pop(&mut self) -> f64 {
        debug_assert!(self.top > 0, "stack underflow");
        self.top -= 1;
        // SAFETY: compiler::symbolic::resolve_bytecode() validates via
        // ByteCode::max_stack_depth's checked_sub that no opcode sequence pops
        // more values than have been pushed (i.e. the stack depth never goes
        // negative). Producing concrete bytecode is the only way to reach the
        // VM, so this guarantees top > 0 before every pop at runtime; an
        // underflow is reported as a compile Err and the program never runs.
        // The debug_assert is a belt-and-suspenders check.
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

/// The three chunk-shaped f64 regions a static view can be read from, plus the
/// two pieces of run state that say what a snapshot read means before its
/// snapshot exists.
///
/// `temp_storage` is deliberately NOT a field: it is the one region an opcode
/// can also WRITE through while reading views (every array-producing opcode
/// does), so it stays a separate `&mut` parameter and only the read side is
/// bundled here. Every field is `Copy`, so a caller mints one for the length of
/// a read loop and drops it before touching `curr`/`temp_storage` mutably.
#[derive(Clone, Copy)]
pub(crate) struct ChunkRegions<'a> {
    curr: &'a [f64],
    /// The snapshot taken after the previous step's stocks (`PREVIOUS`).
    prev: &'a [f64],
    /// The snapshot taken after the initials phase (`INIT`).
    initial: &'a [f64],
    /// Mirrors `EvalState::use_prev_fallback`: true until the first
    /// `prev_values` snapshot exists.
    use_prev_fallback: bool,
    /// Which phase is being evaluated, so an `Initial` view resolves its
    /// "during initials the snapshot is not taken yet" branch exactly as
    /// `Opcode::LoadInitial` does.
    part: StepPart,
}

impl<'a> ChunkRegions<'a> {
    /// A bundle over `curr` alone, for test harnesses that build a
    /// `ViewStorage::Curr` view by hand and address no snapshot region. The
    /// snapshot slices are deliberately EMPTY rather than aliases of `curr`, so
    /// a view that mis-routes to one panics instead of quietly returning
    /// plausible values.
    #[cfg(test)]
    pub(crate) fn curr_only(curr: &'a [f64]) -> Self {
        ChunkRegions {
            curr,
            prev: &[],
            initial: &[],
            use_prev_fallback: false,
            part: StepPart::Flows,
        }
    }

    /// The slice a view's elements live in and the flat base to add its
    /// `offset`/`flat_offset` to, or `None` when every element of the view reads
    /// the `PREVIOUS` fallback instead of a buffer.
    ///
    /// The `None` case is the array route's half of the first-step semantics,
    /// and it is deliberately a BRANCH rather than a reliance on `prev_values`
    /// being zero-filled: `Opcode::LoadPrev` returns its caller-supplied
    /// fallback while `use_prev_fallback` is set, and an array-valued
    /// `PREVIOUS` can only carry the default fallback of 0
    /// (`codegen::is_default_previous_fallback` rejects any other), so element
    /// for element the two routes agree by construction. Making it a branch is
    /// also what keeps the wasm backend -- whose `reset` does not clear the
    /// snapshot regions -- able to mirror this with the same `select` its
    /// scalar `LoadPrev` already emits.
    #[inline]
    fn backing<'s>(
        &self,
        view: &RuntimeView,
        temp_storage: &'s [f64],
        context: &ByteCodeContext,
    ) -> Option<(&'s [f64], usize)>
    where
        'a: 's,
    {
        match view.storage {
            ViewStorage::Curr => Some((self.curr, view.base_off as usize)),
            ViewStorage::Temp => Some((temp_storage, context.temp_offsets[view.base_off as usize])),
            ViewStorage::Prev => {
                if self.use_prev_fallback {
                    None
                } else {
                    Some((self.prev, view.base_off as usize))
                }
            }
            // During initials the snapshot has not been captured yet, so read
            // `curr` -- which IS the initial value being computed. Mirrors
            // `Opcode::LoadInitial`.
            ViewStorage::Initial => {
                let data = if self.part == StepPart::Initials {
                    self.curr
                } else {
                    self.initial
                };
                Some((data, view.base_off as usize))
            }
        }
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
    // Snapshot of curr[] after t=0 initials; used by LoadInitial opcode.
    initial_values: &'a [f64],
    // Snapshot of curr[] taken after stocks but before the time advance
    // each timestep; used by LoadPrev in the following iteration.
    prev_values: &'a mut [f64],
    // When true, LoadPrev returns the per-callsite fallback instead of
    // reading prev_values. True until the first prev_values snapshot
    // has been taken, then false for the rest of the simulation.
    use_prev_fallback: bool,
}

impl CompiledSlicedSimulation {
    /// Build the indexed module table from the keyed `CompiledModule` map,
    /// resolving every module declaration's `(model_name, input_set)` key to a
    /// child index so the hot eval loop never reconstructs or hashes a key.
    fn build(modules: &HashMap<ModuleKey, CompiledModule>, root: &ModuleKey) -> Self {
        // Stable, deterministic ordering so module indices don't depend on
        // HashMap iteration order.
        let mut keys: Vec<&ModuleKey> = modules.keys().collect();
        keys.sort();

        let key_to_idx: HashMap<ModuleKey, u32> = keys
            .iter()
            .enumerate()
            .map(|(idx, key)| ((*key).clone(), idx as u32))
            .collect();

        let resolved: Vec<ResolvedModule> = keys
            .iter()
            .map(|key| {
                let m = &modules[*key];
                // Resolve each child declaration's key to its module index.
                let child_targets: Vec<u32> = m
                    .context
                    .modules
                    .iter()
                    .map(|decl| {
                        let child_key = make_module_key(&decl.model_name, &decl.input_set);
                        key_to_idx[&child_key]
                    })
                    .collect();
                // 3-address fusion (R2): fold leaf operand loads into the
                // binary ops of the per-timestep flows/stocks programs. Done
                // on the Vm's execution copy (not the cached CompiledModule,
                // which stays a pure symbolizable artifact) so the fused
                // opcodes never re-enter the symbolic layer. make_mut clones
                // the bytecode out of the shared Arc once per Vm; the scan is
                // linear and cheap relative to a simulation run. Initials run
                // once and their AssignCurr targets are read elsewhere, so they
                // are left unfused.
                let mut flows = m.compiled_flows.clone();
                let mut stocks = m.compiled_stocks.clone();
                Arc::make_mut(&mut flows).fuse_three_address();
                Arc::make_mut(&mut stocks).fuse_three_address();
                ResolvedModule {
                    ident: m.ident.clone(),
                    context: m.context.clone(),
                    initials: m.compiled_initials.clone(),
                    flows,
                    stocks,
                    child_targets,
                }
            })
            .collect();

        let root_idx = key_to_idx[root] as usize;
        CompiledSlicedSimulation {
            modules: resolved,
            root_idx,
            key_to_idx,
        }
    }
}

/// Recursively collect all bytecode locations for simple constants (AssignConstCurr opcodes)
/// that appear in a module's flows bytecode and its submodules.
///
/// Only offsets with AssignConstCurr in the flows phase are considered overridable
/// (stocks with constant initials are not overridable). For each such offset,
/// ALL bytecode locations across flows, stocks, and initials are collected so
/// that a single `set_value` call mutates every literal that feeds that offset.
///
/// The special conveyor/queue build path subsequently RETRACTS the pass-written
/// slots from this set (`CompiledSimulation::exclude_overridable_offsets` /
/// `Vm::set_conveyor_plans` / `Vm::set_queue_plans`): a pass-driven flow's
/// placeholder `0` matches the AssignConstCurr rule here, but the per-step pass
/// overwrites its slot, so an override on it must reject rather than silently
/// do nothing (GH #871).
fn collect_constant_info(
    modules: &HashMap<ModuleKey, CompiledModule>,
    module_key: &ModuleKey,
    base_off: usize,
) -> HashMap<usize, Vec<BytecodeLocation>> {
    let mut result: HashMap<usize, Vec<BytecodeLocation>> = HashMap::new();
    let Some(module) = modules.get(module_key) else {
        return result;
    };

    // First pass: identify which absolute offsets are overridable (flows only).
    let mut flow_offsets: HashMap<usize, Vec<BytecodeLocation>> = HashMap::new();
    for op in module.compiled_flows.code.iter() {
        if let Opcode::AssignConstCurr { off, literal_id } = op {
            let abs_off = base_off + *off as usize;
            flow_offsets
                .entry(abs_off)
                .or_default()
                .push(BytecodeLocation::FlowOrStock {
                    module_key: module_key.clone(),
                    part: StepPart::Flows,
                    literal_id: *literal_id,
                });
        }
    }

    // Second pass: for each overridable offset, also collect locations in stocks and initials.
    let mut all_locations: HashMap<usize, Vec<BytecodeLocation>> = HashMap::new();

    for op in module.compiled_stocks.code.iter() {
        if let Opcode::AssignConstCurr { off, literal_id } = op {
            let abs_off = base_off + *off as usize;
            if flow_offsets.contains_key(&abs_off) {
                all_locations
                    .entry(abs_off)
                    .or_default()
                    .push(BytecodeLocation::FlowOrStock {
                        module_key: module_key.clone(),
                        part: StepPart::Stocks,
                        literal_id: *literal_id,
                    });
            }
        }
    }

    for (idx, initial) in module.compiled_initials.iter().enumerate() {
        for op in initial.bytecode.code.iter() {
            if let Opcode::AssignConstCurr { off, literal_id } = op {
                let abs_off = base_off + *off as usize;
                if flow_offsets.contains_key(&abs_off) {
                    all_locations
                        .entry(abs_off)
                        .or_default()
                        .push(BytecodeLocation::Initial {
                            module_key: module_key.clone(),
                            initial_index: idx,
                            literal_id: *literal_id,
                        });
                }
            }
        }
    }

    // Merge: flows first, then stocks/initials for each offset.
    for (abs_off, mut flow_locs) in flow_offsets {
        if let Some(extra) = all_locations.remove(&abs_off) {
            flow_locs.extend(extra);
        }
        result.entry(abs_off).or_default().extend(flow_locs);
    }

    // Recurse into submodules.
    for module_decl in &module.context.modules {
        let child_key = make_module_key(&module_decl.model_name, &module_decl.input_set);
        let child_base = base_off + module_decl.off;
        for (abs_off, locations) in collect_constant_info(modules, &child_key, child_base) {
            result.entry(abs_off).or_default().extend(locations);
        }
    }

    result
}

/// Collect absolute offsets of all stock variables by scanning stock-phase
/// bytecode. Recurses into child modules via `EvalModule` to capture
/// submodule internals (SMOOTH/DELAY stocks).
fn collect_stock_offsets(
    modules: &HashMap<ModuleKey, CompiledModule>,
    key: &ModuleKey,
    base_off: usize,
) -> Vec<usize> {
    let module = match modules.get(key) {
        Some(m) => m,
        None => return Vec::new(),
    };
    let mut offsets = Vec::new();
    for op in module.compiled_stocks.code.iter() {
        match op {
            Opcode::BinOpAssignNext { off, .. } => {
                offsets.push(base_off + *off as usize);
            }
            Opcode::EvalModule { id, .. } => {
                let decl = &module.context.modules[*id as usize];
                let child_key = make_module_key(&decl.model_name, &decl.input_set);
                offsets.extend(collect_stock_offsets(
                    modules,
                    &child_key,
                    base_off + decl.off,
                ));
            }
            _ => {}
        }
    }
    // Defensive dedup: duplicates would cause double-counted derivatives.
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Advance a multi-dimensional index in row-major order. Shared by all
/// vector operation opcodes to iterate over array elements.
#[inline]
pub(crate) fn increment_indices(indices: &mut [u16], dims: &[u16]) {
    for d in (0..indices.len()).rev() {
        indices[d] += 1;
        if indices[d] < dims[d] {
            break;
        }
        indices[d] = 0;
    }
}

/// Sentinel written into the `next` chunk by `poison_next_chunk_for_test`. A
/// distinctive finite value rather than NaN, so a slot that carries forward is
/// distinguishable from a model's own NaN.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub const POISON_SENTINEL: f64 = -1.234567e123;

impl Vm {
    pub fn new(sim: CompiledSimulation) -> Result<Vm> {
        if sim.specs.stop < sim.specs.start {
            return sim_err!(
                BadSimSpecs,
                "end time has to be after start time".to_string()
            );
        }
        // Strict positivity: reject dt <= 0 (and NaN), but accept any positive
        // value including very small ones (e.g. 1e-8 in f32).  Using approx_eq
        // here would incorrectly reject small-but-valid timesteps.
        if sim.specs.dt <= 0.0 || sim.specs.dt.is_nan() {
            return sim_err!(BadSimSpecs, "dt must be greater than 0".to_string());
        }

        let root_module = sim.modules.get(&sim.root).ok_or_else(|| {
            Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some("compiled simulation missing root module".to_string()),
            )
        })?;
        let n_slots = root_module.n_slots;
        let n_chunks = sim.specs.n_chunks;
        let data: Box<[f64]> = vec![0.0; n_slots * (n_chunks + 2)].into_boxed_slice();

        // Allocate temp storage based on context temp info
        let temp_total_size = root_module.context.temp_total_size;
        let temp_storage = vec![0.0; temp_total_size];

        // Collect stock offsets for RK integration (empty for Euler)
        let stock_offsets = match sim.specs.method {
            Method::Euler => Vec::new(),
            Method::RungeKutta2 | Method::RungeKutta4 => {
                collect_stock_offsets(&sim.modules, &sim.root, 0)
            }
        };
        let rk_scratch = vec![0.0; stock_offsets.len() * 2];

        let sliced_sim = CompiledSlicedSimulation::build(&sim.modules, &sim.root);

        Ok(Vm {
            specs: sim.specs,
            offsets: sim.offsets,
            sliced_sim,
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
            constant_info: sim.cached_constant_info,
            original_literals: HashMap::new(),
            initial_values: vec![0.0; n_slots].into_boxed_slice(),
            prev_values: vec![0.0; n_slots].into_boxed_slice(),
            stock_offsets,
            rk_scratch,
            prev_values_valid: false,
            #[cfg(any(test, feature = "test-support"))]
            poison_next: false,
            conveyor_plans: Vec::new(),
            conveyors: Vec::new(),
            conveyor_last_unit: i64::MIN,
            queue_plans: Vec::new(),
            queues: Vec::new(),
            coupling: crate::queue_compile::CouplingTable::default(),
        })
    }

    /// Attach resolved conveyor plans (docs/design/conveyors.md §9.3). Called by
    /// the unified special-stock build path ([`crate::queue_compile::build_vm`])
    /// after ordinary compilation resolves the belt parameter/flow slots. A
    /// plain `Vm::new` leaves the plan list empty, so ordinary models are
    /// unaffected.
    pub fn set_conveyor_plans(&mut self, plans: Vec<crate::conveyor_compile::ConveyorPlan>) {
        self.conveyor_last_unit = self.specs.start.floor() as i64;
        // Pass-written slots (driven outflows, leaks, containers) must not be
        // overridable: their placeholder `0` compiles to AssignConstCurr, but
        // the conveyor pass overwrites them every step, so an accepted
        // override would be silently ineffective (GH #871). The build path
        // already retracts them from the compiled sim's constant info (so the
        // no-VM `is_constant_offset` check agrees); repeating the retraction
        // here makes a Vm assembled directly from an unscrubbed
        // CompiledSimulation reject too.
        for plan in &plans {
            for off in plan.pass_written_offsets() {
                self.constant_info.remove(&off);
            }
        }
        self.conveyor_plans = plans;
        // Re-derive the coupling table from the (possibly updated) plan pair:
        // the coupling is compile-time constant, so it is computed here once
        // instead of every Euler step (GH #878).
        self.coupling =
            crate::queue_compile::CouplingTable::build(&self.conveyor_plans, &self.queue_plans);
    }

    /// Attach resolved queue plans (docs/design/queues.md §10.3). Called by the
    /// queue-aware build path ([`crate::queue_compile::build_vm`]) after ordinary
    /// compilation resolves the queue stock/flow slots. A plain `Vm::new` leaves
    /// the plan list empty, so ordinary models are unaffected. The FIFO side
    /// table is (re)built in `run_initials`, so nothing else is set here.
    pub fn set_queue_plans(&mut self, plans: Vec<crate::queue_compile::QueuePlan>) {
        // Same pass-written override retraction as set_conveyor_plans (GH
        // #871): the queue pass owns the driven outflow + container slots.
        for plan in &plans {
            for off in plan.pass_written_offsets() {
                self.constant_info.remove(&off);
            }
        }
        self.queue_plans = plans;
        // Same attach-time coupling-table derivation as set_conveyor_plans:
        // both setters rebuild it so the result is independent of the order
        // the two plan sets are attached in.
        self.coupling =
            crate::queue_compile::CouplingTable::build(&self.conveyor_plans, &self.queue_plans);
    }

    /// Test-support: fill the `next` chunk PAST the implicit-global prefix with
    /// a sentinel at the top of every Euler step, before the Flows phase runs.
    ///
    /// Exposes which slots carry information across a step: anything not
    /// rewritten by the Flows or Stocks phase surfaces as the sentinel in the
    /// saved results. The prefix is deliberately preserved -- `Expr::Dt` lowers
    /// to a `curr[DT_OFF]` read inside every stock update, so poisoning it
    /// corrupts every stock and hides the property under test. See
    /// `only_documented_classes_carry_across_a_step`.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)] // test-support: used by tests/integration/simulate.rs
    pub fn poison_next_chunk_for_test(&mut self) {
        self.poison_next = true;
    }

    pub fn run_to_end(&mut self) -> Result<()> {
        let end = self.specs.stop;
        self.run_to(end)
    }

    #[inline(never)]
    pub fn run_to(&mut self, end: f64) -> Result<()> {
        // Conveyors integrate under Euler only: the slat model is defined per-DT
        // and has no meaning under Runge-Kutta substeps (§9.4). The build path
        // rejects this at compile time; this guards a Vm assembled directly.
        if !self.conveyor_plans.is_empty() && !matches!(self.specs.method, Method::Euler) {
            return sim_err!(
                ConveyorNonEulerMethod,
                "conveyors require Euler integration".to_string()
            );
        }
        // Queues are Euler-only for the same reason (docs/design/queues.md §10.3):
        // the per-DT admit-then-serve model has no meaning under RK substeps.
        if !self.queue_plans.is_empty() && !matches!(self.specs.method, Method::Euler) {
            return sim_err!(
                QueueNonEulerMethod,
                "queues require Euler integration".to_string()
            );
        }
        self.run_initials()?;

        let spec_start = self.specs.start;
        let dt = self.specs.dt;
        let save_step = self.specs.save_step;
        let n_slots = self.n_slots;
        let n_chunks = self.n_chunks;

        let save_every = std::cmp::max(1, (save_step / dt).round() as usize);

        self.stack.clear();
        let mut data = self.data.take().unwrap();

        let root_idx = self.sliced_sim.root_idx;

        self.view_stack.clear();
        self.iter_stack.clear();
        self.broadcast_stack.clear();

        // Split RK scratch buffers before borrowing other fields for EvalState.
        // For Euler these are empty slices (zero cost).
        let n_stocks = self.stock_offsets.len();
        let stock_offsets: &[usize] = &self.stock_offsets;
        let (saved, accum) = self.rk_scratch.split_at_mut(n_stocks);

        let mut state = EvalState {
            stack: &mut self.stack,
            temp_storage: &mut self.temp_storage,
            view_stack: &mut self.view_stack,
            iter_stack: &mut self.iter_stack,
            broadcast_stack: &mut self.broadcast_stack,
            initial_values: &self.initial_values,
            prev_values: &mut self.prev_values,
            // Tells LoadPrev to return the fallback until the first
            // prev_values snapshot is taken.  Tracked in Vm so that
            // segmented run_to() calls don't reset it.
            use_prev_fallback: !self.prev_values_valid,
        };

        // Macro for the save/advance logic shared by all integration methods.
        // Placed here because it captures local variables from run_to.
        // NOTE: contains `break` that exits the enclosing `loop` in each
        // integration method arm -- the caller must be inside a loop.
        macro_rules! save_advance {
            ($data:expr) => {{
                self.step_accum += 1;
                let (curr_sa, _) =
                    borrow_two(&mut $data, n_slots, self.curr_chunk, self.next_chunk);
                let is_initial_timestep =
                    (self.curr_chunk == 0) && (curr_sa[TIME_OFF] == spec_start);
                if self.step_accum != save_every && !is_initial_timestep {
                    let (curr2, next2) =
                        borrow_two(&mut $data, n_slots, self.curr_chunk, self.next_chunk);
                    curr2.copy_from_slice(next2);
                } else {
                    self.curr_chunk = self.next_chunk;
                    if self.next_chunk + 1 >= n_chunks + 2 {
                        break;
                    }
                    self.next_chunk += 1;
                    self.step_accum = 0;
                }
            }};
        }

        #[cfg(any(test, feature = "test-support"))]
        let poison_next = self.poison_next;

        match self.specs.method {
            Method::Euler => loop {
                let (curr, next) = borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
                if curr[TIME_OFF] > end {
                    break;
                }
                #[cfg(any(test, feature = "test-support"))]
                if poison_next {
                    next[IMPLICIT_VAR_COUNT..].fill(POISON_SENTINEL);
                }

                if self.conveyor_plans.is_empty() && self.queue_plans.is_empty() {
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                } else {
                    // Conveyors and/or queues run their native passes between the
                    // Flows and Stocks phases (empty plan lists make each a no-op,
                    // so a model with only one kind pays nothing for the other).
                    //
                    // Publish conveyor container-access values (§10) at STEP-START,
                    // before the flows phase: they reflect the belt as left by the
                    // previous step (= start-of-step) and, because each container
                    // variable is a no-flow stock, survive the flows/stocks phases
                    // unchanged so Flows-phase readers see the start-of-step value.
                    crate::conveyor_compile::publish_container_values(
                        &self.conveyor_plans,
                        &self.conveyors,
                        curr,
                    );
                    // Publish queue container-access values (§8) likewise at
                    // step-start: the batch state as left by the previous step's
                    // admit/serve, surviving the flows/stocks phases in a no-flow
                    // stock so `SUM(queue)` etc. read start-of-step batches.
                    crate::queue_compile::publish_queue_container_values(
                        &self.queue_plans,
                        &self.queues,
                        curr,
                    );
                    // Flows compute the pass inputs (belt transit/capacity/leak
                    // fractions and requested inflow rates; queue inflow rates),
                    // the passes advance the side tables and write the driven flow
                    // rates, then Stocks integrate every stock -- including the
                    // conveyor and queue stocks -- from those rates
                    // (docs/design/conveyors.md §4.3/§9.3, docs/design/queues.md
                    // §4.1/§9). `run_coupled_passes` runs the conveyor and queue
                    // passes; when queues feed a discrete conveyor it interleaves
                    // each coupled queue's serve (in the belt's inflow declaration
                    // order) between that conveyor's phase A and phase B, and
                    // otherwise delegates to the two independent passes
                    // byte-identically.
                    Self::eval(
                        &self.sliced_sim,
                        &mut state,
                        root_idx,
                        StepPart::Flows,
                        0,
                        &[],
                        curr,
                        next,
                    );
                    let t = curr[TIME_OFF];
                    // A mid-run <sample> re-latch that would exceed the belt's
                    // slat-count bound (§4.1) surfaces here as a loud simulation
                    // error rather than a silent geometry clamp or an OOM/abort;
                    // restore the data buffer before propagating (as init_belts
                    // does), so the Vm stays reusable.
                    if let Err((code, msg)) = crate::queue_compile::run_coupled_passes(
                        &self.conveyor_plans,
                        &mut self.conveyors,
                        &self.queue_plans,
                        &mut self.queues,
                        &self.coupling,
                        curr,
                        dt,
                        t,
                        spec_start,
                        &mut self.conveyor_last_unit,
                    ) {
                        self.data = Some(data);
                        return Err(Error::new(ErrorKind::Simulation, code, Some(msg)));
                    }
                    Self::eval(
                        &self.sliced_sim,
                        &mut state,
                        root_idx,
                        StepPart::Stocks,
                        0,
                        &[],
                        curr,
                        next,
                    );
                }
                state.prev_values.copy_from_slice(curr);
                state.use_prev_fallback = false;
                self.prev_values_valid = true;
                next[TIME_OFF] = curr[TIME_OFF] + dt;

                save_advance!(data);
            },
            Method::RungeKutta4 => {
                loop {
                    let (curr, next) =
                        borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
                    if curr[TIME_OFF] > end {
                        break;
                    }

                    let saved_time = curr[TIME_OFF];

                    // Stage 1: evaluate at (t, y)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s1 = next[off] - curr[off];
                        saved[i] = curr[off];
                        accum[i] = s1;
                        curr[off] = saved[i] + s1 * 0.5;
                    }
                    curr[TIME_OFF] = saved_time + dt * 0.5;

                    // Stage 2: evaluate at (t + dt/2, y + s1/2)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s2 = next[off] - curr[off];
                        accum[i] += 2.0 * s2;
                        curr[off] = saved[i] + s2 * 0.5;
                    }

                    // Stage 3: evaluate at (t + dt/2, y + s2/2)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s3 = next[off] - curr[off];
                        accum[i] += 2.0 * s3;
                        curr[off] = saved[i] + s3;
                    }
                    curr[TIME_OFF] = saved_time + dt;

                    // Stage 4: evaluate at (t + dt, y + s3)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s4 = next[off] - curr[off];
                        accum[i] += s4;
                        // Final RK4 combination: y_{n+1} = y_n + (s1 + 2*s2 + 2*s3 + s4) / 6
                        next[off] = saved[i] + accum[i] / 6.0;
                        curr[off] = saved[i]; // restore original
                    }

                    curr[TIME_OFF] = saved_time;
                    next[TIME_OFF] = saved_time + dt;

                    // Re-evaluate flows (not stocks) with the restored state
                    // so that curr has correct aux/flow output values.
                    // Stages 2-4 overwrote them with trial-point evaluations.
                    // An alternative would be saving/restoring all non-stock
                    // slots, but that's ~n_slots copies vs one flow eval --
                    // the re-eval is simpler and the cost is bounded (one
                    // extra flow eval on top of the 4 stage evals).
                    Self::eval(
                        &self.sliced_sim,
                        &mut state,
                        root_idx,
                        StepPart::Flows,
                        0,
                        &[],
                        curr,
                        next,
                    );
                    // Snapshot AFTER re-eval so PREVIOUS() in the next timestep
                    // sees the correct state at time t.
                    state.prev_values.copy_from_slice(curr);
                    state.use_prev_fallback = false;
                    self.prev_values_valid = true;

                    save_advance!(data);
                }
            }
            Method::RungeKutta2 => {
                loop {
                    let (curr, next) =
                        borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
                    if curr[TIME_OFF] > end {
                        break;
                    }

                    let saved_time = curr[TIME_OFF];

                    // Stage 1: evaluate at (t, y)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s1 = next[off] - curr[off];
                        saved[i] = curr[off];
                        accum[i] = s1;
                        curr[off] = saved[i] + s1; // full Euler step for trial
                    }
                    curr[TIME_OFF] = saved_time + dt;

                    // Stage 2: evaluate at (t + dt, y + s1)
                    Self::eval_step(&self.sliced_sim, &mut state, root_idx, curr, next);
                    for (i, &off) in stock_offsets.iter().enumerate() {
                        let s2 = next[off] - curr[off];
                        accum[i] += s2;
                        // Heun's method: y_{n+1} = y_n + (s1 + s2) / 2
                        next[off] = saved[i] + accum[i] / 2.0;
                        curr[off] = saved[i]; // restore original
                    }

                    curr[TIME_OFF] = saved_time;
                    next[TIME_OFF] = saved_time + dt;

                    // Re-evaluate flows with restored state (see RK4 comment)
                    Self::eval(
                        &self.sliced_sim,
                        &mut state,
                        root_idx,
                        StepPart::Flows,
                        0,
                        &[],
                        curr,
                        next,
                    );
                    state.prev_values.copy_from_slice(curr);
                    state.use_prev_fallback = false;
                    self.prev_values_valid = true;

                    save_advance!(data);
                }
            }
        }

        // The integration loop breaks on `curr[TIME] > end` *after* an advance, so
        // the live curr chunk holds the resting-point stocks + reserved time vars
        // but its flow/aux/constant slots were never recomputed for the advanced
        // time -- Euler's `curr.copy_from_slice(next)` leaves a stale `next` row,
        // and the chunk-ring advance lands on a chunk whose non-stock slots are
        // stale (e.g. 0 for a constant). A mid-run `get_value_now` of a non-stock
        // would otherwise read that garbage. Re-evaluate root flows once at the
        // resting curr so the chunk is fully self-consistent ("the value at the
        // current time") and identical to the wasm backend's resting curr (#625).
        //
        // This touches only the live curr chunk: every results row was already
        // saved (and `get_series` reads chunks `[0, curr_chunk)`, excluding this
        // one), a resumed `run_to` re-evaluates from scratch, and `run_to_end`
        // reads the last *results* row -- so it is invisible to the saved series,
        // to resume, and to a full run. It does NOT re-snapshot `prev_values`, so a
        // resume's `PREVIOUS` still sees the last completed step.
        //
        // Guarded on `curr_chunk != next_chunk`: when `run_to(target)` is called
        // with `target` past FINAL_TIME the loop exits via the chunk-ring
        // exhaustion break in `save_advance!`, which sets `curr_chunk = next_chunk`
        // before breaking. Calling `borrow_two` with two equal chunk indices would
        // slice out of bounds and panic. That exhausted-slab case is exactly the
        // one a mid-run read never reaches (a full slab means time has reached
        // FINAL_TIME, not mid-interval), so skipping the re-eval there is correct
        // and matches the pre-#625 graceful clamp.
        if self.curr_chunk != self.next_chunk {
            let (curr, next) = borrow_two(&mut data, n_slots, self.curr_chunk, self.next_chunk);
            // For a conveyor/queue model the Flows re-eval alone would UNDO the
            // self-consistency it exists to provide: each pass-driven flow
            // (primary outflow, leak, queue outflow) compiles to a placeholder
            // `AssignConstCurr 0` that the per-step pass overwrites, so re-running
            // Flows would stamp 0 into those slots, and the container stocks
            // would keep the value published at the START of the last completed
            // step (one step stale relative to the side tables). Mirror the step
            // prologue instead: publish start-of-step container values from the
            // real side tables, re-eval Flows, then run the coupled passes as a
            // side-effect-free PREVIEW on CLONED side tables (and a cloned
            // `conveyor_last_unit`) -- re-running the pass on the real state
            // would double-advance the belts/FIFOs when the run resumes. The
            // preview writes exactly what the resumed step will recompute and
            // save for this time, so a mid-run read equals the eventually-saved
            // row. (A `set_value` AFTER this run_to lands only in the override
            // literals + its own slot; like every other derived value in the
            // resting chunk, the pass-driven slots reflect it on the next
            // run_to, when the whole prologue re-runs.)
            if !self.conveyor_plans.is_empty() || !self.queue_plans.is_empty() {
                crate::conveyor_compile::publish_container_values(
                    &self.conveyor_plans,
                    &self.conveyors,
                    curr,
                );
                crate::queue_compile::publish_queue_container_values(
                    &self.queue_plans,
                    &self.queues,
                    curr,
                );
            }
            Self::eval(
                &self.sliced_sim,
                &mut state,
                root_idx,
                StepPart::Flows,
                0,
                &[],
                curr,
                next,
            );
            if !self.conveyor_plans.is_empty() || !self.queue_plans.is_empty() {
                // Snapshot the chunk so a failed preview (a mid-run <sample>
                // re-latch exceeding the slat bound, §4.1) restores the plain
                // post-Flows state instead of leaving half-written pass output.
                // The preview is presentation-only, so the error is NOT
                // propagated here: this run_to completed everything the caller
                // asked for, and the same error will surface loudly from the
                // step itself if the run resumes without the inputs changing.
                let snapshot = curr.to_vec();
                let mut conveyors = self.conveyors.clone();
                let mut queues = self.queues.clone();
                let mut last_unit = self.conveyor_last_unit;
                let t = curr[TIME_OFF];
                if crate::queue_compile::run_coupled_passes(
                    &self.conveyor_plans,
                    &mut conveyors,
                    &self.queue_plans,
                    &mut queues,
                    &self.coupling,
                    curr,
                    dt,
                    t,
                    spec_start,
                    &mut last_unit,
                )
                .is_err()
                {
                    curr.copy_from_slice(&snapshot);
                }
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

    #[cfg(test)]
    pub(crate) fn stock_offsets(&self) -> &[usize] {
        &self.stock_offsets
    }

    /// Returns whether a given absolute data-buffer offset corresponds to a
    /// simple constant (AssignConstCurr opcode), O(1) lookup against precomputed map.
    fn is_constant(&self, off: usize) -> bool {
        self.constant_info.contains_key(&off)
    }

    /// Resolve a `ModuleKey` (carried by a `BytecodeLocation` from the
    /// constant-info map) to its module index. Cold path only -- used by the
    /// `set_value` / `clear_values` literal-override machinery, never the hot
    /// eval loop.
    fn module_idx_for(&self, module_key: &ModuleKey) -> usize {
        *self
            .sliced_sim
            .key_to_idx
            .get(module_key)
            .expect("module key must exist") as usize
    }

    /// Read the current value of a literal at a bytecode location.
    fn read_literal(&self, loc: &BytecodeLocation) -> f64 {
        match loc {
            BytecodeLocation::FlowOrStock {
                module_key,
                part,
                literal_id,
            } => {
                let module = &self.sliced_sim.modules[self.module_idx_for(module_key)];
                let bytecode = match part {
                    StepPart::Flows => &module.flows,
                    StepPart::Stocks => &module.stocks,
                    StepPart::Initials => unreachable!(),
                };
                bytecode.literals[*literal_id as usize]
            }
            BytecodeLocation::Initial {
                module_key,
                initial_index,
                literal_id,
            } => {
                let module = &self.sliced_sim.modules[self.module_idx_for(module_key)];
                module.initials[*initial_index].bytecode.literals[*literal_id as usize]
            }
        }
    }

    /// Write a value to the literal at a bytecode location, using Arc::make_mut
    /// for copy-on-write semantics on shared bytecode.
    fn write_literal(&mut self, loc: &BytecodeLocation, value: f64) {
        match loc {
            BytecodeLocation::FlowOrStock {
                module_key,
                part,
                literal_id,
            } => {
                let idx = self.module_idx_for(module_key);
                let module = &mut self.sliced_sim.modules[idx];
                let bytecode = match part {
                    StepPart::Flows => &mut module.flows,
                    StepPart::Stocks => &mut module.stocks,
                    StepPart::Initials => unreachable!(),
                };
                Arc::make_mut(bytecode).literals[*literal_id as usize] = value;
            }
            BytecodeLocation::Initial {
                module_key,
                initial_index,
                literal_id,
            } => {
                let idx = self.module_idx_for(module_key);
                let module = &mut self.sliced_sim.modules[idx];
                let initials = Arc::make_mut(&mut module.initials);
                initials[*initial_index].bytecode.literals[*literal_id as usize] = value;
            }
        }
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
        self.prev_values.fill(0.0);
        self.temp_storage.fill(0.0);
        self.stack.clear();
        self.view_stack.clear();
        self.iter_stack.clear();
        self.broadcast_stack.clear();
        self.rk_scratch.fill(0.0);
        self.prev_values_valid = false;
    }

    /// Apply an override for a constant at the given absolute offset.
    /// Named constants get their own literal slots at compile time (via
    /// push_named_literal), so no de-interning is needed at runtime.
    fn apply_override(&mut self, off: usize, value: f64) {
        // Clone locations once; we need ownership because write_literal borrows &mut self.
        let locations = self.constant_info[&off].clone();
        if !self.original_literals.contains_key(&off) {
            let originals: Vec<_> = locations
                .iter()
                .map(|loc| (loc.clone(), self.read_literal(loc)))
                .collect();
            self.original_literals.insert(off, originals);
        }
        for loc in &locations {
            self.write_literal(loc, value);
        }
        self.set_value_now(off, value);
    }

    /// Set a value override for a simple constant by canonical variable name.
    /// Mutates the bytecode literals directly so AssignConstCurr needs no branching.
    /// Returns the data-buffer offset of the variable on success.
    pub fn set_value(&mut self, ident: &Ident<Canonical>, value: f64) -> Result<usize> {
        let off = match self.offsets.get(ident) {
            Some(&off) => off,
            None => {
                return sim_err!(
                    DoesNotExist,
                    format!("variable '{}' not found in offsets map", ident.as_str())
                );
            }
        };
        if !self.is_constant(off) {
            return sim_err!(
                BadOverride,
                format!(
                    "cannot set value of '{}': not a simple constant",
                    ident.as_str()
                )
            );
        }
        self.apply_override(off, value);
        Ok(off)
    }

    /// Set a value override for a simple constant by raw data-buffer offset.
    pub fn set_value_by_offset(&mut self, off: usize, value: f64) -> Result<()> {
        if off >= self.n_slots {
            return sim_err!(
                BadOverride,
                format!("offset {} out of bounds (n_slots={})", off, self.n_slots)
            );
        }
        if !self.is_constant(off) {
            return sim_err!(
                BadOverride,
                format!("cannot set value of offset {}: not a simple constant", off)
            );
        }
        self.apply_override(off, value);
        Ok(())
    }

    /// Remove all value overrides, restoring original compiled literal values.
    pub fn clear_values(&mut self) {
        let drained: Vec<_> = self.original_literals.drain().collect();
        for (_off, originals) in drained {
            for (loc, original_value) in originals {
                self.write_literal(&loc, original_value);
            }
        }
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
        let module_inputs: &[f64] = &[];
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
            // During initials, LoadInitial falls back to curr[] (which IS the
            // initial value being computed). The snapshot hasn't been captured yet.
            initial_values: &self.initial_values,
            prev_values: &mut self.prev_values,
            // prev_values hasn't been populated yet during initials.
            use_prev_fallback: true,
        };

        Self::eval_initials(
            &self.sliced_sim,
            &mut state,
            self.sliced_sim.root_idx,
            0,
            module_inputs,
            curr,
            next,
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

        // Capture a snapshot of curr[] after the initials phase for INIT(x).
        // The initial_values buffer preserves t=0 values across all timesteps.
        let curr_start = self.curr_chunk * self.n_slots;
        self.initial_values
            .copy_from_slice(&data[curr_start..curr_start + self.n_slots]);

        // Initialize the conveyor belts (docs/design/conveyors.md §7/§9.3). The
        // belt parameters (transit, capacity, leak fractions) are synthesized
        // auxes that nothing depends on, so they are NOT in the initials runlist
        // -- they are computed only in the flows phase. Run one flows evaluation
        // here (AFTER the initial_values snapshot above, so INIT() stays pure)
        // to populate those slots, then read the transit / stock <eqn> / initial
        // fractions to fill the belts.
        if !self.conveyor_plans.is_empty() {
            let root_idx = self.sliced_sim.root_idx;
            // A fresh EvalState (the one above borrowed `initial_values`, which
            // the snapshot just re-borrowed mutably). `use_prev_fallback` is
            // true: no prev snapshot exists yet during initialization.
            let mut init_state = EvalState {
                stack: &mut self.stack,
                temp_storage: &mut self.temp_storage,
                view_stack: &mut self.view_stack,
                iter_stack: &mut self.iter_stack,
                broadcast_stack: &mut self.broadcast_stack,
                initial_values: &self.initial_values,
                prev_values: &mut self.prev_values,
                use_prev_fallback: true,
            };
            let (curr, next) =
                borrow_two(&mut data, self.n_slots, self.curr_chunk, self.next_chunk);
            Self::eval(
                &self.sliced_sim,
                &mut init_state,
                root_idx,
                StepPart::Flows,
                0,
                &[],
                curr,
                next,
            );
            match crate::conveyor_compile::init_belts(&self.conveyor_plans, curr, dt) {
                Ok(states) => self.conveyors = states,
                Err((code, msg)) => {
                    self.data = Some(data);
                    return Err(Error::new(ErrorKind::Simulation, code, Some(msg)));
                }
            }
            // Publish container-access values (§10) for the initialized belts, so
            // the t=0 slot holds the start-of-step value even before the first
            // Euler step re-publishes it. (The container-reading initials are
            // re-run against these published values below.)
            crate::conveyor_compile::publish_container_values(
                &self.conveyor_plans,
                &self.conveyors,
                curr,
            );
            self.conveyor_last_unit = spec_start.floor() as i64;
        }

        // Initialize the queue side tables (docs/design/queues.md §7). Unlike
        // conveyors, a queue needs NO extra Flows evaluation: its only dynamic
        // init input is the stock's initial value `V`, which the initials pass
        // already wrote into `curr[stock_off]` (the stock's own `<eqn>` is in the
        // initials runlist). Reads `curr` only (never mutates it), so its
        // placement after the `initial_values` snapshot keeps INIT() pure.
        if !self.queue_plans.is_empty() {
            let (curr, _next) =
                borrow_two(&mut data, self.n_slots, self.curr_chunk, self.next_chunk);
            self.queues = crate::queue_compile::init_queues(&self.queue_plans, curr);
            // Publish container-access values (§8) for the initialized queues, so
            // the t=0 slot holds the start-of-step value even before the first
            // Euler step re-publishes it (mirrors the conveyor init publish).
            crate::queue_compile::publish_queue_container_values(
                &self.queue_plans,
                &self.queues,
                curr,
            );
        }

        // Reconcile INIT(<container access>) with the published start-of-run
        // container values. The `initial_values` snapshot above deliberately
        // precedes the belt/queue init (belt init needs the Flows-phase belt
        // params AND the stock's initial value, so it cannot run before the
        // snapshot -- and keeping the snapshot first is what makes INIT() of
        // ordinary variables pure). But that means the snapshot captured each
        // container stock's frozen '0' <eqn> placeholder, not the belt/FIFO's
        // start-of-run total. Any initials-phase reader of a container slot --
        // INIT(SUM(belt)) directly, INIT(SUM(belt[a])) via its synthesized
        // per-element helper aux, or a stock initialized from SUM(belt) -- thus
        // captured 0 (yielding inf/NaN in ratios like SUM(belt)/INIT(SUM(belt))).
        //
        // The belt/queue passes just published the true container values into
        // `curr`, so re-run the initials runlist over `curr` and re-snapshot: every
        // container-dependent initial now recomputes from the published values.
        // The container stocks themselves are SKIPPED (their init is the '0'
        // placeholder; re-running it would clobber the published value before the
        // dependent reads run), so the published `curr` slots survive the pass and
        // feed the reads. This is idempotent for every other variable (the initials
        // runlist is deterministic and topologically complete, so a second pass
        // recomputes identical values), and re-running with an empty skip set for a
        // container-free model would be a no-op -- so the pass is gated on the
        // presence of slots needing reconciliation. `prev_values` needs no
        // analogous fix: PREVIOUS() returns its fallback on the first step
        // (use_prev_fallback -- correct, no prior step exists), and thereafter
        // prev_values is seeded only by run_to's end-of-step copy_from_slice,
        // which already captures the no-flow container slot.
        //
        // A §7.2 explicit-list conveyor stock joins the skip set defensively:
        // its compiled <eqn> is the expansion-time NORMALIZED-total
        // placeholder (conveyor_compile::normalized_init_total runs the same
        // fill init_belts does), so init_belts' write-back normally changes
        // nothing -- but if the two ever diverged, skipping the stock keeps
        // the belt-derived total authoritative and the re-run + re-snapshot
        // propagate it to dependent initials and INIT().
        let reconcile_skip_offsets: std::collections::HashSet<usize> = self
            .conveyor_plans
            .iter()
            .flat_map(|p| p.containers.iter())
            .chain(self.queue_plans.iter().flat_map(|p| p.containers.iter()))
            .map(|c| c.off)
            .chain(
                self.conveyor_plans
                    .iter()
                    .filter(|p| p.init_values.is_some())
                    .map(|p| p.stock_off),
            )
            .collect();
        if !reconcile_skip_offsets.is_empty() {
            let root_idx = self.sliced_sim.root_idx;
            let mut init_state = EvalState {
                stack: &mut self.stack,
                temp_storage: &mut self.temp_storage,
                view_stack: &mut self.view_stack,
                iter_stack: &mut self.iter_stack,
                broadcast_stack: &mut self.broadcast_stack,
                initial_values: &self.initial_values,
                prev_values: &mut self.prev_values,
                use_prev_fallback: true,
            };
            let (curr, next) =
                borrow_two(&mut data, self.n_slots, self.curr_chunk, self.next_chunk);
            Self::eval_initials_skipping(
                &self.sliced_sim,
                &mut init_state,
                root_idx,
                0,
                module_inputs,
                curr,
                next,
                &reconcile_skip_offsets,
            );
            let curr_start = self.curr_chunk * self.n_slots;
            self.initial_values
                .copy_from_slice(&data[curr_start..curr_start + self.n_slots]);
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

    /// Run all per-variable initials for a module (in dependency order).
    #[allow(clippy::too_many_arguments)]
    fn eval_initials(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module_idx: usize,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let module = &sliced_sim.modules[module_idx];
        let context = &module.context;
        for compiled_initial in module.initials.iter() {
            Self::eval_bytecode(
                sliced_sim,
                state,
                context,
                &compiled_initial.bytecode,
                StepPart::Initials,
                module_off,
                module_idx,
                module_inputs,
                curr,
                next,
            );
        }
    }

    /// Re-evaluate the initials runlist, skipping every variable whose
    /// AssignCurr targets are ALL in `skip_offsets`. Used to reconcile
    /// INIT(<container access>) with the belt/queue values published into `curr`
    /// AFTER the primary initials snapshot (see `run_initials`): re-running the
    /// runlist recomputes any container-dependent initial from the published
    /// values, while skipping the container stocks themselves preserves those
    /// published `curr` slots (their init is the frozen '0' placeholder, which
    /// would otherwise clobber the published value before the dependent reads).
    /// A CompiledInitial's `offsets` are exactly the slots it writes, so a
    /// container stock -- which writes only its own slot(s) -- is the one whose
    /// offsets are wholly contained in the container-slot set.
    // Mirrors `eval_initials`' parameter list (the borrow split between `curr` and
    // `next` is what keeps it a free function rather than a `&mut self` method) plus
    // the one skip set; bundling them would obscure that correspondence.
    #[allow(clippy::too_many_arguments)]
    fn eval_initials_skipping(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module_idx: usize,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
        skip_offsets: &std::collections::HashSet<usize>,
    ) {
        let module = &sliced_sim.modules[module_idx];
        let context = &module.context;
        for compiled_initial in module.initials.iter() {
            if !compiled_initial.offsets.is_empty()
                && compiled_initial
                    .offsets
                    .iter()
                    .all(|off| skip_offsets.contains(off))
            {
                continue;
            }
            Self::eval_bytecode(
                sliced_sim,
                state,
                context,
                &compiled_initial.bytecode,
                StepPart::Initials,
                module_off,
                module_idx,
                module_inputs,
                curr,
                next,
            );
        }
    }

    /// Evaluate one full integration step: compute all flows/auxes then
    /// update all stocks.  Used by each RK stage and the Euler loop.
    /// Always evaluates the root module (`module_off == 0`).
    #[inline(always)]
    fn eval_step(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module_idx: usize,
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        Self::eval(
            sliced_sim,
            state,
            module_idx,
            StepPart::Flows,
            0,
            &[],
            curr,
            next,
        );
        Self::eval(
            sliced_sim,
            state,
            module_idx,
            StepPart::Stocks,
            0,
            &[],
            curr,
            next,
        );
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn eval(
        sliced_sim: &CompiledSlicedSimulation,
        state: &mut EvalState<'_>,
        module_idx: usize,
        part: StepPart,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let module = &sliced_sim.modules[module_idx];
        let bytecode = match part {
            StepPart::Flows => &module.flows,
            StepPart::Stocks => &module.stocks,
            StepPart::Initials => unreachable!("initials are evaluated via eval_initials"),
        };
        Self::eval_bytecode(
            sliced_sim,
            state,
            &module.context,
            bytecode,
            part,
            module_off,
            module_idx,
            module_inputs,
            curr,
            next,
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
        // Index of the module currently executing, into
        // `sliced_sim.modules`. Used to resolve `EvalModule` child targets
        // without reconstructing/hashing a module key. `context` is
        // `&sliced_sim.modules[module_idx].context`.
        module_idx: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
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
        let initial_values = state.initial_values;
        let mut prev_values = &mut *state.prev_values;
        let use_prev_fallback = state.use_prev_fallback;

        // The read-only chunk regions a static view can address. Minted fresh
        // at each use rather than bound once: `curr` is `&mut` here and several
        // arms write it (and `temp_storage`) in the same breath as reading a
        // view, so a long-lived shared reborrow would not typecheck.
        macro_rules! regions {
            () => {
                ChunkRegions {
                    curr: &*curr,
                    prev: &*prev_values,
                    initial: initial_values,
                    use_prev_fallback,
                    part,
                }
            };
        }

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
                // LoadPrev returns the caller-provided fallback until
                // prev_values has been populated (i.e., after the first
                // timestep completes).  The use_prev_fallback flag is the
                // sole mechanism -- it replaces the old TIME == INITIAL_TIME
                // check, which broke when RK stages advanced TIME to trial
                // points before prev_values was initialized.
                Opcode::SubVarPrev { l, r, lit } => {
                    let lhs = curr[module_off + *l as usize];
                    let rhs = if use_prev_fallback {
                        bytecode.literals[*lit as usize]
                    } else {
                        prev_values[module_off + *r as usize]
                    };
                    // Through `eval_op2` so the fused form is bit-identical to
                    // the sequence by construction, not by inspection.
                    stack.push(eval_op2(Op2::Sub, lhs, rhs));
                }
                Opcode::BinStackPrev { r, lit, op } => {
                    let rhs = if use_prev_fallback {
                        bytecode.literals[*lit as usize]
                    } else {
                        prev_values[module_off + *r as usize]
                    };
                    let lhs = stack.pop();
                    stack.push(eval_op2(*op, lhs, rhs));
                }
                Opcode::LoadPrevConst { off, lit } => {
                    let value = if use_prev_fallback {
                        bytecode.literals[*lit as usize]
                    } else {
                        prev_values[module_off + *off as usize]
                    };
                    stack.push(value);
                }
                Opcode::ApplyTerConst { func, lit } => {
                    let time = curr[TIME_OFF];
                    let dt = curr[DT_OFF];
                    let c = bytecode.literals[*lit as usize];
                    let b = stack.pop();
                    let a = stack.pop();
                    stack.push(apply(*func, time, dt, a, b, c));
                }
                Opcode::LoadPrev { off } => {
                    let fallback = stack.pop();
                    let value = if use_prev_fallback {
                        fallback
                    } else {
                        prev_values[module_off + *off as usize]
                    };
                    stack.push(value);
                }
                // LoadInitial reads from the initial-value buffer captured at t=0.
                // During the initials phase, the snapshot hasn't been taken yet,
                // so we fall back to curr[] (which IS being initialized).
                Opcode::LoadInitial { off } => {
                    let abs_off = module_off + *off as usize;
                    let value = if part == StepPart::Initials {
                        curr[abs_off]
                    } else {
                        initial_values[abs_off]
                    };
                    stack.push(value);
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
                        initial_values,
                        prev_values,
                        use_prev_fallback,
                    };
                    // Resolve the child module by precomputed index instead of
                    // reconstructing + SipHashing a (model_name, input_set) key.
                    let child_module_off = module_off + context.modules[*id as usize].off;
                    let child_idx =
                        sliced_sim.modules[module_idx].child_targets[*id as usize] as usize;
                    match part {
                        StepPart::Initials => {
                            Self::eval_initials(
                                sliced_sim,
                                &mut child_state,
                                child_idx,
                                child_module_off,
                                &module_inputs,
                                curr,
                                next,
                            );
                        }
                        StepPart::Flows | StepPart::Stocks => {
                            Self::eval(
                                sliced_sim,
                                &mut child_state,
                                child_idx,
                                part,
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
                        initial_values: _,
                        prev_values: pv,
                        use_prev_fallback: _,
                    } = child_state;
                    stack = s;
                    temp_storage = ts;
                    view_stack = vs;
                    iter_stack = is_;
                    broadcast_stack = bs;
                    prev_values = pv;
                }
                Opcode::AssignCurr { off } => {
                    curr[module_off + *off as usize] = stack.pop();
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
                // === CONDITIONAL SELECT (R3) ===
                // The fused `SetCond; If[; AssignCurr]`. Codegen pushes the true
                // arm, then the false arm, then the condition, so these pop in
                // the order cond, false, true -- exactly the order the three
                // separate arms performed them in. Selecting between two
                // already-evaluated operands is what `If` did; nothing about
                // branch evaluation changes here.
                Opcode::SelectIf {} => {
                    let cond = stack.pop();
                    let f = stack.pop();
                    let t = stack.pop();
                    stack.push(if is_truthy(cond) { t } else { f });
                }
                Opcode::SelectIfAssignCurr { off } => {
                    let cond = stack.pop();
                    let f = stack.pop();
                    let t = stack.pop();
                    curr[module_off + *off as usize] = if is_truthy(cond) { t } else { f };
                    debug_assert_eq!(0, stack.len());
                }
                // === LEAF STORES AND MODULE-INPUT OPERANDS (R3) ===
                // Each reads its leaf from the region `LoadVar` / `LoadInitial`
                // / `LoadModuleInput` would have read and writes `curr`
                // directly, touching the arithmetic stack not at all.
                Opcode::AssignVarCurr { src, dst } => {
                    curr[module_off + *dst as usize] = curr[module_off + *src as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignInitialCurr { src, dst } => {
                    // Mirrors `LoadInitial`: during the initials phase the
                    // snapshot does not exist yet, so read the row being built.
                    let abs_src = module_off + *src as usize;
                    let value = if part == StepPart::Initials {
                        curr[abs_src]
                    } else {
                        initial_values[abs_src]
                    };
                    curr[module_off + *dst as usize] = value;
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignModInputCurr { input, dst } => {
                    curr[module_off + *dst as usize] = module_inputs[*input as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::BinStackModInput { r_input, op } => {
                    let lv = stack.pop();
                    let rv = module_inputs[*r_input as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::AssignStackModInputCurr { dst, b_input, op } => {
                    let lhs = stack.pop();
                    let rhs = module_inputs[*b_input as usize];
                    curr[module_off + *dst as usize] = eval_op2(*op, lhs, rhs);
                    debug_assert_eq!(0, stack.len());
                }
                // === 3-ADDRESS BINARY OPS (R2) ===
                // Operands are read straight from curr[]/literals; the *Stack*
                // forms take the lhs from the arithmetic stack. Each pushes the
                // result, replacing a Load;Load;Op2 or Load;Op2 sequence.
                Opcode::BinVarVar { l, r, op } => {
                    let lv = curr[module_off + *l as usize];
                    let rv = curr[module_off + *r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinVarConst { l, r, op } => {
                    let lv = curr[module_off + *l as usize];
                    let rv = bytecode.literals[*r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinConstVar { l, r, op } => {
                    let lv = bytecode.literals[*l as usize];
                    let rv = curr[module_off + *r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinStackVar { r, op } => {
                    let lv = stack.pop();
                    let rv = curr[module_off + *r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinStackConst { r, op } => {
                    let lv = stack.pop();
                    let rv = bytecode.literals[*r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                // === 3-ADDRESS BINARY OPS WITH GLOBAL OPERANDS (R2 extension) ===
                // A `_global` operand is read from `curr[g]` directly (an
                // absolute global slot, NO module_off -- like LoadGlobalVar),
                // while a plain var operand is `curr[module_off + v]` (like
                // LoadVar). The operand order `l op r` matches the original load
                // order (load-bearing for Sub/Div).
                Opcode::BinGlobalVar { l_global, r, op } => {
                    let lv = curr[*l_global as usize];
                    let rv = curr[module_off + *r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinVarGlobal { l, r_global, op } => {
                    let lv = curr[module_off + *l as usize];
                    let rv = curr[*r_global as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinGlobalConst { l_global, r, op } => {
                    let lv = curr[*l_global as usize];
                    let rv = bytecode.literals[*r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinConstGlobal { l, r_global, op } => {
                    let lv = bytecode.literals[*l as usize];
                    let rv = curr[*r_global as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinGlobalGlobal {
                    l_global,
                    r_global,
                    op,
                } => {
                    let lv = curr[*l_global as usize];
                    let rv = curr[*r_global as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                Opcode::BinStackGlobal { r_global, op } => {
                    let lv = stack.pop();
                    let rv = curr[*r_global as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                // Two constant leaves: still evaluated at run time (the operands
                // are two distinct interned literals, not a folded result).
                Opcode::BinConstConst { l, r, op } => {
                    let lv = bytecode.literals[*l as usize];
                    let rv = bytecode.literals[*r as usize];
                    stack.push(eval_op2(*op, lv, rv));
                }
                // === 3-ADDRESS FUSED LEAF ASSIGNMENTS (R2 extension) ===
                // `dst = a op b`, operands read straight from curr[]/literals,
                // result written straight to curr[]/next[]. The operator is in
                // the opcode tag, so each arm is one straight-line f64 op with no
                // `eval_op2` branch. The stack is untouched (the original
                // sequence pushed two operands and popped both into the store),
                // so it must already be empty -- the same invariant the
                // BinOpAssign* arms assert.
                Opcode::AssignAddVarVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] + curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubVarVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] - curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulVarVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] * curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivVarVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] / curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignAddVarVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] + curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubVarVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] - curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulVarVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] * curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivVarVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] / curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignAddVarConstCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] + bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubVarConstCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] - bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulVarConstCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] * bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivVarConstCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        curr[module_off + *l as usize] / bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignAddVarConstNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] + bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubVarConstNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] - bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulVarConstNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] * bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivVarConstNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        curr[module_off + *l as usize] / bytecode.literals[*r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignAddConstVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] + curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubConstVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] - curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulConstVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] * curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivConstVarCurr { l, r, dst } => {
                    curr[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] / curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignAddConstVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] + curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignSubConstVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] - curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignMulConstVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] * curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignDivConstVarNext { l, r, dst } => {
                    next[module_off + *dst as usize] =
                        bytecode.literals[*l as usize] / curr[module_off + *r as usize];
                    debug_assert_eq!(0, stack.len());
                }
                // === 2-ADDRESS STACK-LEAF FUSED ASSIGNMENTS (R2 extension) ===
                // `dst = lhs op b`: pop the pre-existing lhs from the stack,
                // combine it with the leaf rhs (operator in payload), store. The
                // pop leaves the stack empty, matching the BinOpAssign* invariant.
                Opcode::AssignStackVarCurr { dst, b, op } => {
                    let lhs = stack.pop();
                    let rhs = curr[module_off + *b as usize];
                    curr[module_off + *dst as usize] = eval_op2(*op, lhs, rhs);
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignStackVarNext { dst, b, op } => {
                    let lhs = stack.pop();
                    let rhs = curr[module_off + *b as usize];
                    next[module_off + *dst as usize] = eval_op2(*op, lhs, rhs);
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignStackConstCurr { dst, b, op } => {
                    let lhs = stack.pop();
                    let rhs = bytecode.literals[*b as usize];
                    curr[module_off + *dst as usize] = eval_op2(*op, lhs, rhs);
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::AssignStackConstNext { dst, b, op } => {
                    let lhs = stack.pop();
                    let rhs = bytecode.literals[*b as usize];
                    next[module_off + *dst as usize] = eval_op2(*op, lhs, rhs);
                    debug_assert_eq!(0, stack.len());
                }
                Opcode::Apply { func } => {
                    let time = curr[TIME_OFF];
                    let dt = curr[DT_OFF];
                    // Pop exactly the operands this builtin reads. Codegen
                    // pushes `BuiltinId::arity()` of them and no padding, so an
                    // unread operand is never on the stack to begin with; the
                    // value handed to `apply` for an unread position is
                    // arbitrary, and 0.0 keeps it deterministic.
                    let arity = func.arity();
                    let c = if arity >= 3 { stack.pop() } else { 0.0 };
                    let b = if arity >= 2 { stack.pop() } else { 0.0 };
                    let a = if arity >= 1 { stack.pop() } else { 0.0 };

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
                    if element_offset < 0.0 || element_offset >= *table_count as usize as f64 {
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
                // The element offset was resolved and bounds-checked at emit
                // time, so this reads `graphical_functions[base_gf + elem]`
                // with no pop and no range check -- the two things the general
                // `Lookup` arm above spends its extra dispatch on.
                Opcode::LookupDirect {
                    base_gf,
                    elem,
                    mode,
                } => {
                    let lookup_index = stack.pop();
                    let gf = &context.graphical_functions[*base_gf as usize + *elem as usize];
                    let result = match mode {
                        LookupMode::Interpolate => lookup(gf, lookup_index),
                        LookupMode::Forward => lookup_forward(gf, lookup_index),
                        LookupMode::Backward => lookup_backward(gf, lookup_index),
                    };
                    stack.push(result);
                }
                Opcode::Ret => {
                    break;
                }

                // =========================================================
                // VIEW STACK OPERATIONS
                // =========================================================
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
                    view_stack.push(static_view.to_runtime_view(module_off as u32));
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

                    // Pre-compute flat offsets for iteration. A dense linear
                    // run (contiguous, or an offset slice like `arr[2, *]`)
                    // needs no precompute: LoadIterElement's direct path
                    // computes `view.offset + current`, which is exactly
                    // `dense_linear_start() + current`.
                    let flat_offsets = if view.dense_linear_start().is_some() {
                        None
                    } else {
                        // Need to pre-compute all flat offsets
                        let mut offsets = Vec::with_capacity(size);
                        let dims = &view.dims;
                        let n_dims = dims.len();
                        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];

                        for _ in 0..size {
                            offsets.push(view.flat_offset(&indices));
                            increment_indices(&mut indices, dims);
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
                            // Dense linear run: flat offset = dense_linear_start()
                            // + current, and dense_linear_start() == view.offset.
                            view.offset as usize + iter_state.current
                        };

                        let value = Self::read_view_element(
                            view,
                            flat_off,
                            regions!(),
                            temp_storage,
                            context,
                        );
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
                        let result = if source_view.same_shape(iter_view) {
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
                            let value = Self::read_view_element(
                                source_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
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
                        let result = if source_view.same_shape(iter_view) {
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
                            let value = Self::read_view_element(
                                source_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
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
                // Empty views return 0.0 for SUM (the additive identity)
                Opcode::ArraySum {} => {
                    let view = view_stack.last().unwrap();
                    let sum = Self::reduce_view(
                        temp_storage,
                        view,
                        regions!(),
                        context,
                        |acc, v| acc + v,
                        0.0,
                    );
                    stack.push(sum);
                }

                Opcode::ArrayMax {} => {
                    let view = view_stack.last().unwrap();
                    if view.size() == 0 {
                        stack.push(f64::NAN);
                    } else {
                        let max = Self::reduce_view(
                            temp_storage,
                            view,
                            regions!(),
                            context,
                            |acc, v| if v > acc { v } else { acc },
                            f64::NEG_INFINITY,
                        );
                        stack.push(max);
                    }
                }

                Opcode::ArrayMin {} => {
                    let view = view_stack.last().unwrap();
                    if view.size() == 0 {
                        stack.push(f64::NAN);
                    } else {
                        let min = Self::reduce_view(
                            temp_storage,
                            view,
                            regions!(),
                            context,
                            |acc, v| if v < acc { v } else { acc },
                            f64::INFINITY,
                        );
                        stack.push(min);
                    }
                }

                Opcode::ArrayMean {} => {
                    let view = view_stack.last().unwrap();
                    if view.size() == 0 {
                        stack.push(f64::NAN);
                    } else {
                        let sum = Self::reduce_view(
                            temp_storage,
                            view,
                            regions!(),
                            context,
                            |acc, v| acc + v,
                            0.0,
                        );
                        let count = view.size() as f64;
                        stack.push(sum / count);
                    }
                }

                Opcode::ArrayStddev {} => {
                    let view = view_stack.last().unwrap();
                    let size = view.size();
                    if size == 0 {
                        stack.push(f64::NAN);
                    } else {
                        let sum = Self::reduce_view(
                            temp_storage,
                            view,
                            regions!(),
                            context,
                            |acc, v| acc + v,
                            0.0,
                        );
                        let fsize = size as f64;
                        let mean = sum / fsize;

                        // Second pass for variance
                        let variance_sum = Self::reduce_view(
                            temp_storage,
                            view,
                            regions!(),
                            context,
                            |acc, v| acc + (v - mean).powf(2.0),
                            0.0,
                        );
                        let stddev = (variance_sum / fsize).sqrt();
                        stack.push(stddev);
                    }
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

                        let value = Self::read_view_element(
                            view,
                            flat_off,
                            regions!(),
                            temp_storage,
                            context,
                        );
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
                        increment_indices(&mut bc_state.result_indices, &bc_state.result_dims);

                        // Jump backward to loop start
                        pc = (pc as isize + *jump_back as isize) as usize;
                        continue; // Skip pc increment
                    }
                    // else: iteration done, continue to next opcode
                }

                Opcode::EndBroadcastIter {} => {
                    broadcast_stack.pop();
                }

                // =========================================================
                // VECTOR OPERATIONS
                // =========================================================
                Opcode::VectorSelect {} => {
                    let action = stack.pop().round() as i32;
                    let max_value = stack.pop();

                    let expr_view = &view_stack[view_stack.len() - 1];
                    let sel_view = &view_stack[view_stack.len() - 2];

                    if !sel_view.is_valid || !expr_view.is_valid {
                        stack.push(f64::NAN);
                    } else {
                        // Zip semantics: stop at the shorter array
                        let size = sel_view.size().min(expr_view.size());
                        let n_dims = sel_view.dims.len();

                        let mut selected: SmallVec<[f64; 32]> = SmallVec::new();
                        let mut sel_indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];
                        let mut expr_indices: SmallVec<[u16; 4]> =
                            smallvec::smallvec![0; expr_view.dims.len()];

                        for _ in 0..size {
                            let sel_off = sel_view.flat_offset(&sel_indices);
                            let sel_val = Self::read_view_element(
                                sel_view,
                                sel_off,
                                regions!(),
                                temp_storage,
                                context,
                            );

                            if is_truthy(sel_val) {
                                let expr_off = expr_view.flat_offset(&expr_indices);
                                let expr_val = Self::read_view_element(
                                    expr_view,
                                    expr_off,
                                    regions!(),
                                    temp_storage,
                                    context,
                                );
                                selected.push(expr_val);
                            }

                            increment_indices(&mut sel_indices, &sel_view.dims);
                            increment_indices(&mut expr_indices, &expr_view.dims);
                        }

                        let result = if selected.is_empty() {
                            max_value
                        } else {
                            match action {
                                1 => selected.iter().cloned().fold(f64::INFINITY, f64::min),
                                2 => selected.iter().sum::<f64>() / selected.len() as f64,
                                3 => selected.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                                4 => selected.iter().product(),
                                _ => selected.iter().sum(),
                            }
                        };
                        stack.push(result);
                    }
                }

                // Genuine-Vensim VECTOR ELM MAP -- rule + citations on
                // `crate::vm_vector_elm_map::vector_elm_map` (no modulo;
                // OOB/NaN => `:NA:`).
                Opcode::VectorElmMap {
                    write_temp_id,
                    full_source_len,
                } => {
                    let offset_view = &view_stack[view_stack.len() - 1];
                    let source_view = &view_stack[view_stack.len() - 2];
                    crate::vm_vector_elm_map::vector_elm_map(
                        source_view,
                        offset_view,
                        *write_temp_id,
                        *full_source_len,
                        regions!(),
                        temp_storage,
                        context,
                    );
                }

                // Genuine-Vensim VECTOR SORT ORDER -- per-iterated-slice
                // (per-row) 0-based ranks; rule + citations on
                // `crate::vm_vector_sort_order::vector_sort_order`.
                Opcode::VectorSortOrder { write_temp_id } => {
                    let direction = stack.pop().round() as i32;
                    let input_view = &view_stack[view_stack.len() - 1];
                    crate::vm_vector_sort_order::vector_sort_order(
                        input_view,
                        direction,
                        *write_temp_id,
                        regions!(),
                        temp_storage,
                        context,
                    );
                }

                Opcode::Rank { write_temp_id } => {
                    let direction = stack.pop().round() as i32;

                    let input_view = &view_stack[view_stack.len() - 1];

                    if !input_view.is_valid {
                        Self::fill_temp_nan(temp_storage, context, *write_temp_id);
                    } else {
                        let size = input_view.size();
                        let n_dims = input_view.dims.len();

                        // Collect (value, original_index) pairs
                        let mut indexed: SmallVec<[(f64, usize); 32]> =
                            SmallVec::with_capacity(size);
                        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];
                        for i in 0..size {
                            let flat_off = input_view.flat_offset(&indices);
                            let val = Self::read_view_element(
                                input_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
                            indexed.push((val, i));
                            increment_indices(&mut indices, &input_view.dims);
                        }

                        if direction == 1 {
                            indexed.sort_by(|a, b| {
                                a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        } else {
                            indexed.sort_by(|a, b| {
                                b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }

                        // Write each element's rank at its original position
                        let temp_off = context.temp_offsets[*write_temp_id as usize];
                        for (rank_0based, &(_, orig_idx)) in indexed.iter().enumerate() {
                            temp_storage[temp_off + orig_idx] = (rank_0based + 1) as f64;
                        }
                    }
                }

                Opcode::LookupArray {
                    base_gf,
                    table_count,
                    mode,
                    write_temp_id,
                } => {
                    // Per-element arrayed-GF lookup (GH #580 Bug B): for each
                    // element `i` of the arrayed GF's view, evaluate that
                    // element's table at the shared scalar `index`. The base
                    // array's *values* are irrelevant (a graphical function is
                    // a pure table); the view supplies the element count and
                    // each element's flat offset into the per-element-table
                    // run `graphical_functions[base_gf .. base_gf + table_count]`
                    // (laid out in declared element order by
                    // `Compiler::table_base_ids`, exactly as the scalar
                    // `Lookup`'s element offset). An out-of-range element index
                    // yields NaN, matching the scalar `Lookup` opcode's bound.
                    let index = stack.pop();
                    let input_view = &view_stack[view_stack.len() - 1];
                    let temp_off = context.temp_offsets[*write_temp_id as usize];

                    if !input_view.is_valid {
                        Self::fill_temp_nan(temp_storage, context, *write_temp_id);
                    } else {
                        let size = input_view.size();
                        let n_dims = input_view.dims.len();
                        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];
                        for i in 0..size {
                            let elem_off = input_view.flat_offset(&indices);
                            let result = if elem_off >= *table_count as usize {
                                f64::NAN
                            } else {
                                let gf = &context.graphical_functions[*base_gf as usize + elem_off];
                                match mode {
                                    LookupMode::Interpolate => lookup(gf, index),
                                    LookupMode::Forward => lookup_forward(gf, index),
                                    LookupMode::Backward => lookup_backward(gf, index),
                                }
                            };
                            temp_storage[temp_off + i] = result;
                            increment_indices(&mut indices, &input_view.dims);
                        }
                    }
                }

                Opcode::AllocateAvailable { write_temp_id } => {
                    let avail = stack.pop();

                    let profile_view = &view_stack[view_stack.len() - 1];
                    let requests_view = &view_stack[view_stack.len() - 2];

                    if !requests_view.is_valid || !profile_view.is_valid {
                        Self::fill_temp_nan(temp_storage, context, *write_temp_id);
                    } else {
                        // Collect request values
                        let req_size = requests_view.size();
                        let req_n_dims = requests_view.dims.len();
                        let mut requests: SmallVec<[f64; 32]> = SmallVec::with_capacity(req_size);
                        let mut req_indices: SmallVec<[u16; 4]> =
                            smallvec::smallvec![0; req_n_dims];
                        for _ in 0..req_size {
                            let flat_off = requests_view.flat_offset(&req_indices);
                            let val = Self::read_view_element(
                                requests_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
                            requests.push(val);
                            increment_indices(&mut req_indices, &requests_view.dims);
                        }

                        let n = requests.len();

                        // Collect profile values (flat 2D array: n requesters x pp_cols)
                        let pp_size = profile_view.size();
                        let pp_n_dims = profile_view.dims.len();
                        let mut pp_values: SmallVec<[f64; 32]> = SmallVec::with_capacity(pp_size);
                        let mut pp_indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; pp_n_dims];
                        for _ in 0..pp_size {
                            let flat_off = profile_view.flat_offset(&pp_indices);
                            let val = Self::read_view_element(
                                profile_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
                            pp_values.push(val);
                            increment_indices(&mut pp_indices, &profile_view.dims);
                        }

                        // Build profile tuples from flat array
                        let pp_cols = if !pp_values.is_empty() && n > 0 && pp_size.is_multiple_of(n)
                        {
                            pp_size / n
                        } else {
                            4
                        };

                        let mut profiles: SmallVec<[(f64, f64, f64, f64); 32]> =
                            SmallVec::with_capacity(n);
                        for i in 0..n {
                            let base = i * pp_cols;
                            let ptype = if base < pp_values.len() {
                                pp_values[base]
                            } else {
                                0.0
                            };
                            let ppriority = if base + 1 < pp_values.len() {
                                pp_values[base + 1]
                            } else {
                                0.0
                            };
                            let pwidth = if base + 2 < pp_values.len() {
                                pp_values[base + 2]
                            } else {
                                1.0
                            };
                            let pextra = if base + 3 < pp_values.len() {
                                pp_values[base + 3]
                            } else {
                                0.0
                            };
                            profiles.push((ptype, ppriority, pwidth, pextra));
                        }

                        let result = allocate_available(&requests, &profiles, avail);

                        let temp_off = context.temp_offsets[*write_temp_id as usize];
                        for (i, &val) in result.iter().enumerate() {
                            temp_storage[temp_off + i] = val;
                        }
                    }
                }

                Opcode::AllocateByPriority { write_temp_id } => {
                    // Pops supply and width from the stack (supply was pushed last, so popped first)
                    let supply = stack.pop();
                    let width = stack.pop();

                    let priority_view = &view_stack[view_stack.len() - 1];
                    let requests_view = &view_stack[view_stack.len() - 2];

                    if !requests_view.is_valid || !priority_view.is_valid {
                        Self::fill_temp_nan(temp_storage, context, *write_temp_id);
                    } else {
                        // Collect request values
                        let req_size = requests_view.size();
                        let req_n_dims = requests_view.dims.len();
                        let mut requests: SmallVec<[f64; 32]> = SmallVec::with_capacity(req_size);
                        let mut req_indices: SmallVec<[u16; 4]> =
                            smallvec::smallvec![0; req_n_dims];
                        for _ in 0..req_size {
                            let flat_off = requests_view.flat_offset(&req_indices);
                            let val = Self::read_view_element(
                                requests_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
                            requests.push(val);
                            increment_indices(&mut req_indices, &requests_view.dims);
                        }

                        let n = requests.len();

                        // Collect priority values
                        let pri_size = priority_view.size();
                        let pri_n_dims = priority_view.dims.len();
                        let mut priorities: SmallVec<[f64; 32]> = SmallVec::with_capacity(pri_size);
                        let mut pri_indices: SmallVec<[u16; 4]> =
                            smallvec::smallvec![0; pri_n_dims];
                        for _ in 0..pri_size {
                            let flat_off = priority_view.flat_offset(&pri_indices);
                            let val = Self::read_view_element(
                                priority_view,
                                flat_off,
                                regions!(),
                                temp_storage,
                                context,
                            );
                            priorities.push(val);
                            increment_indices(&mut pri_indices, &priority_view.dims);
                        }

                        // Construct rectangular priority profiles:
                        // (ptype=1, ppriority=priority[i], pwidth=width, pextra=0)
                        let mut profiles: SmallVec<[(f64, f64, f64, f64); 32]> =
                            SmallVec::with_capacity(n);
                        for i in 0..n {
                            let ppriority = if i < priorities.len() {
                                priorities[i]
                            } else {
                                0.0
                            };
                            profiles.push((1.0, ppriority, width, 0.0));
                        }

                        let result = allocate_available(&requests, &profiles, supply);

                        let temp_off = context.temp_offsets[*write_temp_id as usize];
                        for (i, &val) in result.iter().enumerate() {
                            temp_storage[temp_off + i] = val;
                        }
                    }
                }
            }

            pc += 1;
        }
    }

    /// Helper: Reduce all elements of a view using a fold function
    fn reduce_view<Fold>(
        temp_storage: &[f64],
        view: &RuntimeView,
        regions: ChunkRegions<'_>,
        context: &ByteCodeContext,
        f: Fold,
        init: f64,
    ) -> f64
    where
        Fold: Fn(f64, f64) -> f64,
    {
        // Return NaN for invalid views
        if !view.is_valid {
            return f64::NAN;
        }

        let size = view.size();

        let Some((data, base)) = regions.backing(view, temp_storage, context) else {
            // A PREVIOUS view before the first snapshot: every element is the
            // fallback 0, so fold that many zeros rather than reading a buffer.
            // Same iteration count and same order, so the FP result matches a
            // zero-filled region exactly.
            let mut acc = init;
            for _ in 0..size {
                acc = f(acc, 0.0);
            }
            return acc;
        };

        // Dense linear run (the overwhelmingly common case: whole arrays and
        // leading-dimension slices): fold over the backing slice directly,
        // skipping the per-element index decompose + stride dot product.
        // Iteration order is identical to the general path (row-major ==
        // ascending flat offset for a linear run), so FP reduction results are
        // bit-identical.
        if let Some(start) = view.dense_linear_start() {
            let mut acc = init;
            for &value in &data[base + start..base + start + size] {
                acc = f(acc, value);
            }
            return acc;
        }

        let dims = &view.dims;
        let n_dims = dims.len();

        let mut acc = init;
        let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];

        for _ in 0..size {
            let flat_off = view.flat_offset(&indices);
            acc = f(acc, data[base + flat_off]);
            increment_indices(&mut indices, dims);
        }

        acc
    }

    /// Read a single element from a RuntimeView at a pre-computed memory offset.
    /// Routes through the view's [`ViewStorage`], so a temp view reads
    /// `temp_storage` and a snapshot view reads `prev_values`/`initial_values`.
    /// The `flat_off` parameter is the actual memory offset within the view's storage,
    /// NOT a sequential iteration index. For contiguous views, flat_off equals the
    /// iteration index. For non-contiguous or sparse views, the caller must compute
    /// flat_off via `view.flat_offset(&indices)` or `view.offset_for_iter_index(iter_idx)`.
    #[inline]
    pub(crate) fn read_view_element(
        view: &RuntimeView,
        flat_off: usize,
        regions: ChunkRegions<'_>,
        temp_storage: &[f64],
        context: &ByteCodeContext,
    ) -> f64 {
        match regions.backing(view, temp_storage, context) {
            Some((data, base)) => data[base + flat_off],
            None => 0.0,
        }
    }

    /// Fill a temp storage region with NaN. Uses `temp_offsets` to determine
    /// the correct region size, independent of potentially-invalid runtime views.
    pub(crate) fn fill_temp_nan(
        temp_storage: &mut [f64],
        context: &ByteCodeContext,
        temp_id: TempId,
    ) {
        let idx = temp_id as usize;
        let start = context.temp_offsets[idx];
        let end = context
            .temp_offsets
            .get(idx + 1)
            .copied()
            .unwrap_or(context.temp_total_size);
        for slot in &mut temp_storage[start..end] {
            *slot = f64::NAN;
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
        BuiltinId::Quantum => {
            let x = a;
            let q = b;
            if q == 0.0 { x } else { (x / q).trunc() * q }
        }
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
        // ROUND is Python round() / IEEE roundTiesToEven: nearest integer,
        // exact .5 ties to the even neighbor. `round_ties_even` is also what
        // wasm's single `f64.nearest` instruction computes, so the wasm
        // backend mirrors this bit for bit (see `wasmgen/lower.rs`).
        BuiltinId::Round => a.round_ties_even(),
        BuiltinId::SafeDiv => {
            // Use exact zero comparison, not approx_eq: a denominator that
            // is very small but non-zero (e.g. subnormal) should still
            // produce a / b, not silently fall back to the default c.
            if b != 0.0 { a / b } else { c }
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
        BuiltinId::Sshape => {
            let x = a;
            let bottom = b;
            let top = c;
            bottom + (top - bottom) / (1.0 + (-4.0 * (2.0 * x - 1.0)).exp())
        }
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

// `pub(crate)` so the wasm backend's lookup-helper tests can compare the
// emitted helpers directly against the VM functions they reproduce
// (`wasmgen::lookup`), the byte-faithful oracle for `vm.rs:3055-3186`.
#[inline(never)]
pub(crate) fn lookup(table: &[(f64, f64)], index: f64) -> f64 {
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
    if crate::float::approx_eq(table[i].0, index) {
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
pub(crate) fn lookup_forward(table: &[(f64, f64)], index: f64) -> f64 {
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
pub(crate) fn lookup_backward(table: &[(f64, f64)], index: f64) -> f64 {
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

    /// AC1.6 regression pin (#590): the regular (Interpolate-mode) `lookup`
    /// clamps an out-of-range index to the endpoint y-value and returns NaN for
    /// a NaN index. The lookup-only saved-value fix only changes *which*
    /// expression is fed to a standalone graphical-function variable's table
    /// (the index), never the table's clamp/NaN behavior -- this pins that the
    /// clamp the fix relies on is unchanged.
    #[test]
    fn test_regular_lookup_outside_range_clamps_to_endpoints() {
        let table = test_table(); // x in [0, 2], y = x
        // Below the first x: clamp to the first y.
        assert_eq!(0.0, lookup(&table, -5.0));
        // Above the last x: clamp to the last y.
        assert_eq!(2.0, lookup(&table, 5.0));
        // NaN index: NaN result.
        assert!(lookup(&table, f64::NAN).is_nan());
    }
}

#[cfg(test)]
mod apply_tests {
    use super::*;
    use crate::bytecode::BuiltinId;

    // ── SafeDiv ─────────────────────────────────────────────────────────

    #[test]
    fn safediv_nonzero_denominator() {
        let result = apply(BuiltinId::SafeDiv, 0.0, 1.0, 10.0, 2.0, 99.0);
        assert_eq!(result, 5.0);
    }

    #[test]
    fn safediv_exact_zero_denominator_returns_default() {
        let result = apply(BuiltinId::SafeDiv, 0.0, 1.0, 10.0, 0.0, 99.0);
        assert_eq!(result, 99.0);
    }

    #[test]
    fn safediv_subnormal_denominator_divides_normally() {
        // A subnormal (very small but non-zero) denominator must NOT trigger
        // the fallback branch — this is the key semantic distinction between
        // exact-zero and approx_eq checks.
        let subnormal = f64::MIN_POSITIVE / 2.0; // subnormal value
        assert!(subnormal != 0.0, "subnormal should not be exactly zero");
        let result = apply(BuiltinId::SafeDiv, 0.0, 1.0, 10.0, subnormal, 99.0);
        // Should perform division, not return default
        assert_ne!(
            result, 99.0,
            "subnormal denominator should NOT trigger fallback"
        );
        assert_eq!(result, 10.0 / subnormal);
    }

    #[test]
    fn safediv_negative_zero_is_zero() {
        // -0.0 == 0.0 in IEEE 754, so SafeDiv should return default
        let result = apply(BuiltinId::SafeDiv, 0.0, 1.0, 10.0, -0.0, 99.0);
        assert_eq!(result, 99.0, "negative zero should trigger the fallback");
    }

    // ── Sign ────────────────────────────────────────────────────────────

    #[test]
    fn sign_positive() {
        assert_eq!(1.0, apply(BuiltinId::Sign, 0.0, 1.0, 5.0, 0.0, 0.0));
    }

    #[test]
    fn sign_negative() {
        assert_eq!(-1.0, apply(BuiltinId::Sign, 0.0, 1.0, -3.0, 0.0, 0.0));
    }

    #[test]
    fn sign_zero() {
        assert_eq!(0.0, apply(BuiltinId::Sign, 0.0, 1.0, 0.0, 0.0, 0.0));
    }

    // ── Other builtins ──────────────────────────────────────────────────

    #[test]
    fn apply_abs() {
        assert_eq!(3.0, apply(BuiltinId::Abs, 0.0, 1.0, -3.0, 0.0, 0.0));
    }

    #[test]
    fn apply_int_floors() {
        assert_eq!(3.0, apply(BuiltinId::Int, 0.0, 1.0, 3.7, 0.0, 0.0));
        assert_eq!(-4.0, apply(BuiltinId::Int, 0.0, 1.0, -3.2, 0.0, 0.0));
    }

    /// ROUND sends exact .5 ties to the even neighbor (Python round() /
    /// IEEE roundTiesToEven). Driven by the SHARED case table in
    /// `test_common` -- the same rows the wasm parity test and the
    /// end-to-end pipeline test assert -- so the backends cannot drift.
    #[test]
    fn apply_round_ties_to_even() {
        for &(input, expected) in crate::test_common::ROUND_TIES_TO_EVEN_CASES {
            let got = apply(BuiltinId::Round, 0.0, 1.0, input, 0.0, 0.0);
            crate::test_common::assert_round_case(input, got, expected, "vm-apply");
        }
    }

    #[test]
    fn apply_pi() {
        let result = apply(BuiltinId::Pi, 0.0, 1.0, 0.0, 0.0, 0.0);
        assert!((result - std::f64::consts::PI).abs() < 1e-15);
    }

    #[test]
    fn apply_inf() {
        let result = apply(BuiltinId::Inf, 0.0, 1.0, 0.0, 0.0, 0.0);
        assert!(result.is_infinite() && result > 0.0);
    }

    #[test]
    fn apply_trig_round_trip() {
        // sin(asin(0.5)) should be ~0.5
        let asin_val = apply(BuiltinId::Arcsin, 0.0, 1.0, 0.5, 0.0, 0.0);
        let sin_val = apply(BuiltinId::Sin, 0.0, 1.0, asin_val, 0.0, 0.0);
        assert!((sin_val - 0.5).abs() < 1e-15);
    }

    #[test]
    fn apply_log10() {
        let result = apply(BuiltinId::Log10, 0.0, 1.0, 100.0, 0.0, 0.0);
        assert!((result - 2.0).abs() < 1e-15);
    }

    #[test]
    fn apply_ln() {
        let result = apply(BuiltinId::Ln, 0.0, 1.0, std::f64::consts::E, 0.0, 0.0);
        assert!((result - 1.0).abs() < 1e-15);
    }

    #[test]
    fn apply_sqrt() {
        assert_eq!(3.0, apply(BuiltinId::Sqrt, 0.0, 1.0, 9.0, 0.0, 0.0));
    }

    #[test]
    fn apply_max_min() {
        assert_eq!(7.0, apply(BuiltinId::Max, 0.0, 1.0, 3.0, 7.0, 0.0));
        assert_eq!(3.0, apply(BuiltinId::Min, 0.0, 1.0, 3.0, 7.0, 0.0));
    }
}

#[cfg(test)]
mod is_truthy_and_eval_op2_tests {
    use super::*;

    #[test]
    fn is_truthy_zero_is_false() {
        assert!(!is_truthy(0.0));
    }

    #[test]
    fn is_truthy_nonzero_is_true() {
        assert!(is_truthy(1.0));
        assert!(is_truthy(-1.0));
        assert!(is_truthy(0.001));
    }

    #[test]
    fn is_truthy_nan_is_true() {
        // NaN is not approx_eq to zero, so it's truthy
        assert!(is_truthy(f64::NAN));
    }

    #[test]
    fn eval_op2_arithmetic() {
        assert_eq!(5.0, eval_op2(Op2::Add, 2.0, 3.0));
        assert_eq!(-1.0, eval_op2(Op2::Sub, 2.0, 3.0));
        assert_eq!(6.0, eval_op2(Op2::Mul, 2.0, 3.0));
        assert_eq!(2.0, eval_op2(Op2::Div, 6.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Mod, 7.0, 3.0));
        assert_eq!(8.0, eval_op2(Op2::Exp, 2.0, 3.0));
    }

    #[test]
    fn eval_op2_comparison() {
        assert_eq!(1.0, eval_op2(Op2::Gt, 3.0, 2.0));
        assert_eq!(0.0, eval_op2(Op2::Gt, 2.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Gte, 3.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Lt, 2.0, 3.0));
        assert_eq!(0.0, eval_op2(Op2::Lt, 3.0, 2.0));
        assert_eq!(1.0, eval_op2(Op2::Lte, 3.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Eq, 3.0, 3.0));
        assert_eq!(0.0, eval_op2(Op2::Eq, 3.0, 4.0));
    }

    #[test]
    fn eval_op2_logical() {
        assert_eq!(1.0, eval_op2(Op2::And, 1.0, 1.0));
        assert_eq!(0.0, eval_op2(Op2::And, 1.0, 0.0));
        assert_eq!(0.0, eval_op2(Op2::And, 0.0, 1.0));
        assert_eq!(1.0, eval_op2(Op2::Or, 1.0, 0.0));
        assert_eq!(1.0, eval_op2(Op2::Or, 0.0, 1.0));
        assert_eq!(0.0, eval_op2(Op2::Or, 0.0, 0.0));
    }

    #[test]
    fn is_truthy_negative_zero_is_false() {
        assert!(!is_truthy(-0.0));
    }

    #[test]
    fn is_truthy_epsilon_is_falsy() {
        // f64::EPSILON is within ULP tolerance of 0.0
        assert!(!is_truthy(f64::EPSILON));
    }

    #[test]
    fn is_truthy_small_but_not_zero() {
        assert!(is_truthy(0.001));
        assert!(is_truthy(-0.001));
    }

    #[test]
    fn is_truthy_infinity() {
        assert!(is_truthy(f64::INFINITY));
        assert!(is_truthy(f64::NEG_INFINITY));
    }

    #[test]
    fn eval_op2_mod_negative() {
        // rem_euclid always returns a non-negative result
        assert_eq!(2.0, eval_op2(Op2::Mod, -7.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Mod, -2.0, 3.0));
    }

    #[test]
    fn eval_op2_eq_approx() {
        // Values very close together should be considered equal by approx_eq
        assert_eq!(1.0, eval_op2(Op2::Eq, 0.0, -0.0));
        // Values sufficiently far apart should not be equal
        assert_eq!(0.0, eval_op2(Op2::Eq, 1.0, 1.1));
    }

    #[test]
    fn eval_op2_logical_with_nonunit_truthy() {
        // Non-0/1 truthy values should still work correctly
        assert_eq!(1.0, eval_op2(Op2::And, 5.0, -3.0));
        assert_eq!(0.0, eval_op2(Op2::And, 5.0, 0.0));
        assert_eq!(1.0, eval_op2(Op2::Or, 0.0, 42.0));
        assert_eq!(0.0, eval_op2(Op2::Or, 0.0, -0.0));
    }

    #[test]
    fn eval_op2_comparison_equal_values() {
        assert_eq!(0.0, eval_op2(Op2::Gt, 3.0, 3.0));
        assert_eq!(0.0, eval_op2(Op2::Lt, 3.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Gte, 3.0, 3.0));
        assert_eq!(1.0, eval_op2(Op2::Lte, 3.0, 3.0));
    }
}

#[cfg(test)]
mod per_variable_initials_tests {
    use crate::test_common::TestProject;

    #[test]
    fn test_compiled_constant_offsets_sorted_deduped() {
        // Use a model where auxiliary 'rate' is a stock dependency so it
        // appears in the initials runlist.
        let tp = TestProject::new("offsets_test")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("rate", "0.1", None)
            .flow("inflow", "0", None)
            .stock("pop", "rate * 1000", &["inflow"], &[], None);

        let compiled = tp.compile_incremental().expect("compile should succeed");
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
        let vm_results = tp.run_vm_expecting_success();
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

        let vm_results = tp.run_vm_expecting_success();

        // arr[A]=1, arr[B]=2, arr[C]=3, so s = 1+2+3 = 6
        let s_vm = vm_results.get("s").expect("s should exist in VM");
        assert_eq!(s_vm[0], 6.0, "s initial = 6 in VM");

        let expected: &[(&str, f64)] = &[("arr[a]", 1.0), ("arr[b]", 2.0), ("arr[c]", 3.0)];
        for &(element, expected_val) in expected {
            let vm_val = vm_results
                .get(element)
                .unwrap_or_else(|| panic!("{element} should exist in VM results"));
            assert!(
                (vm_val[0] - expected_val).abs() < 1e-10,
                "{element}: expected={expected_val}, vm={}",
                vm_val[0],
            );
        }

        // Verify CompiledInitial offsets for the array variable
        let compiled = tp.compile_incremental().expect("compile should succeed");
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
#[path = "vm_reset_and_run_initials_tests.rs"]
mod vm_reset_and_run_initials_tests;

#[cfg(test)]
#[path = "vm_set_value_tests.rs"]
mod set_value_tests;

#[cfg(test)]
mod stack_tests {
    use super::*;

    #[test]
    fn test_lifo_ordering() {
        let mut s: Stack = Stack::new();
        for i in 0..10 {
            s.push(i as f64);
        }
        for i in (0..10).rev() {
            assert_eq!(i as f64, s.pop());
        }
    }

    #[test]
    fn test_len_tracks_size() {
        let mut s: Stack = Stack::new();
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
        let mut s: Stack = Stack::new();
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
        let mut s: Stack = Stack::new();
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
        let mut s: Stack = Stack::new();
        s.push(1.0);
        s.push(2.0);
        s.clear();
        s.push(42.0);
        assert_eq!(1, s.len());
        assert_eq!(42.0, s.pop());
    }
}

#[cfg(test)]
mod superinstruction_tests {
    use super::*;
    use crate::bytecode::Opcode;
    use crate::test_common::TestProject;

    fn build_vm(tp: &TestProject) -> Vm {
        let compiled = tp.compile_incremental().unwrap();
        Vm::new(compiled).unwrap()
    }

    /// Helper: collect all opcodes from the flow bytecode of the root module.
    fn flow_opcodes(vm: &Vm) -> Vec<&Opcode> {
        let bc = &vm.sliced_sim.modules[vm.sliced_sim.root_idx].flows;
        bc.code.iter().collect()
    }

    /// Helper: collect all opcodes from the stock bytecode of the root module.
    fn stock_opcodes(vm: &Vm) -> Vec<&Opcode> {
        let bc = &vm.sliced_sim.modules[vm.sliced_sim.root_idx].stocks;
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

    // -----------------------------------------------------------------------
    // BinOpAssignCurr: e.g. `births = population * birth_rate`
    //
    // The peephole pass folds `Op2(Mul); AssignCurr` into `BinOpAssignCurr`, but
    // the later 3-address fusion (R2 extension) -- which runs on the Vm's
    // execution copy that `flow_opcodes` reads -- folds the whole leaf assign
    // `result = rate * 2` (a var times a const) into the register-style
    // `AssignMulVarConstCurr`. So `BinOpAssignCurr` is the *intermediate* form;
    // the fused stream carries the operator-specialized leaf assign instead.
    // -----------------------------------------------------------------------

    #[test]
    fn test_binop_assign_curr_fuses_to_leaf_assign() {
        let tp = TestProject::new("binop_model")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("rate", "0.1", None)
            .aux("result", "rate * 2", None)
            .flow("inflow", "0", None)
            .stock("s", "result", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = flow_opcodes(&vm);
        let has_leaf_mul = ops
            .iter()
            .any(|op| matches!(op, Opcode::AssignMulVarConstCurr { .. }));
        assert!(
            has_leaf_mul,
            "`result = rate * 2` should fuse to AssignMulVarConstCurr in the \
             execution stream, got {:?}",
            ops.iter().map(|o| o.name()).collect::<Vec<_>>()
        );
        // The intermediate `BinOpAssignCurr` must NOT survive into the fused
        // stream for an {Add,Sub,Mul,Div} leaf assign.
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Opcode::BinOpAssignCurr { .. })),
            "BinOpAssignCurr should have been superseded by the leaf-assign form"
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
        // stock with only outflow exercises Sub inside the stock update
        let tp = TestProject::new("fused_next_sub")
            .with_sim_time(0.0, 3.0, 1.0)
            .flow("outflow", "5", None)
            .stock("s", "100", &[], &["outflow"], None);
        let vm_results = tp.run_vm().unwrap();
        let vm_s = &vm_results["s"];
        assert!((vm_s[0] - 100.0).abs() < 1e-10, "initial should be 100");
        assert!(
            (vm_s[1] - 95.0).abs() < 1e-10,
            "step 1 should be 95 (100 - 5)"
        );
        assert!(
            (vm_s[2] - 90.0).abs() < 1e-10,
            "step 2 should be 90 (95 - 5)"
        );
        assert!(
            (vm_s[3] - 85.0).abs() < 1e-10,
            "step 3 should be 85 (90 - 5)"
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

        // product = 2*3 = 6, sum = 2+3 = 5, inflow = 11
        // s starts at 0, gains 11 per step
        let vm_s = &vm_results["s"];
        assert!(
            (vm_s[0] - 0.0).abs() < 1e-10,
            "s at step 0 should be 0, got {}",
            vm_s[0]
        );
        assert!(
            (vm_s[1] - 11.0).abs() < 1e-10,
            "s at step 1 should be 11, got {}",
            vm_s[1]
        );
        assert!(
            (vm_s[2] - 22.0).abs() < 1e-10,
            "s at step 2 should be 22, got {}",
            vm_s[2]
        );
        assert!(
            (vm_s[3] - 33.0).abs() < 1e-10,
            "s at step 3 should be 33, got {}",
            vm_s[3]
        );
    }

    // -----------------------------------------------------------------------
    // R2 extension: fused leaf assignments (3->1) and stack-leaf assigns (2->1)
    // -----------------------------------------------------------------------

    /// Asserts the *actual fused opcodes* of the leaf-assign and stack-leaf-assign
    /// forms, not just the numeric result. The fused stream lives on the Vm's
    /// `sliced_sim` execution copy (the one `flow_opcodes` reads). Sub/Div are
    /// non-commutative, so a swapped `l`/`r` encoding would also change the result.
    #[test]
    fn test_fused_leaf_assign_opcodes_present_in_flow_bytecode() {
        let tp = TestProject::new("leaf_assign_opcodes")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux("a", "20", None)
            .aux("b", "5", None)
            .aux("c", "2", None)
            .aux("vv", "a - b", None)
            .aux("dvv", "a / b", None)
            .aux("vc", "a - 3", None)
            .aux("cv", "10 - a", None)
            .aux("sv", "(a - b) - c", None)
            .aux("scn", "(a / b) / 2", None);

        let vm = build_vm(&tp);
        let ops = flow_opcodes(&vm);

        // Leaf-assign forms (3->1) for the top-level binops.
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignSubVarVarCurr { .. })),
            "`vv = a - b` should fuse to AssignSubVarVarCurr"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignDivVarVarCurr { .. })),
            "`dvv = a / b` should fuse to AssignDivVarVarCurr"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignSubVarConstCurr { .. })),
            "`vc = a - 3` should fuse to AssignSubVarConstCurr"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignSubConstVarCurr { .. })),
            "`cv = 10 - a` should fuse to AssignSubConstVarCurr"
        );
        // Stack-leaf forms (2->1) for the outer op of a nested expression.
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignStackVarCurr { op: Op2::Sub, .. })),
            "`sv = (a - b) - c` outer op should fuse to AssignStackVarCurr"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Opcode::AssignStackConstCurr { op: Op2::Div, .. })),
            "`scn = (a / b) / 2` outer op should fuse to AssignStackConstCurr"
        );

        let res = tp.run_vm().unwrap();
        assert_eq!(res["vv"][0], 15.0, "a - b");
        assert_eq!(res["dvv"][0], 4.0, "a / b");
        assert_eq!(res["vc"][0], 17.0, "a - 3");
        assert_eq!(res["cv"][0], -10.0, "10 - a");
        assert_eq!(res["sv"][0], 13.0, "(a - b) - c");
        // Discriminating value for stack-leaf Div order: (20/5)/2 = 2, whereas a
        // swapped 2/(20/5) would be 0.5.
        assert_eq!(res["scn"][0], 2.0, "(a / b) / 2 = (20/5)/2 = 2");
    }

    /// Stock integration `stock' = stock + flow * dt` exercises the *Next* family
    /// of fused assigns in the stock bytecode: guards that a Next variant writes
    /// `next[]` (not `curr[]`) and the integration is numerically correct.
    #[test]
    fn test_fused_leaf_assign_next_in_stock_bytecode() {
        let tp = TestProject::new("leaf_assign_next")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("inflow", "3", None)
            .stock("s", "10", &["inflow"], &[], None);

        let vm = build_vm(&tp);
        let ops = stock_opcodes(&vm);
        // The update `s' = s + inflow*dt` produces some Next-family assign (the
        // exact variant depends on emission order; this guards that *a* fused or
        // unfused Next store is present and the result is exact).
        let has_next_form = ops.iter().any(|op| {
            matches!(
                op,
                Opcode::AssignStackVarNext { .. }
                    | Opcode::AssignStackConstNext { .. }
                    | Opcode::AssignAddVarVarNext { .. }
                    | Opcode::AssignAddConstVarNext { .. }
                    | Opcode::AssignAddVarConstNext { .. }
                    | Opcode::BinOpAssignNext { .. }
            )
        });
        assert!(
            has_next_form,
            "stock integration should produce a Next-family assign, got {:?}",
            ops.iter().map(|o| o.name()).collect::<Vec<_>>()
        );

        // s(0)=10, s(1)=13, s(2)=16.
        let res = tp.run_vm().unwrap();
        assert_eq!(res["s"][0], 10.0);
        assert_eq!(res["s"][1], 13.0);
        assert_eq!(res["s"][2], 16.0);
    }
}

#[cfg(test)]
#[path = "vm_reset_run_to_and_constants_tests.rs"]
mod vm_reset_run_to_and_constants_tests;

/// `ChunkRegions::backing` is where a view's storage region is resolved, and it
/// is the ONE place the VM reproduces the two snapshot semantics the scalar
/// opcodes carry. It is tested directly because it cannot be reached
/// end-to-end: `Vm::new` and `Vm::reset` zero-fill `prev_values`, and the only
/// moments `use_prev_fallback` is set are exactly the moments the buffer is
/// still zeroed -- so the fallback BRANCH and the buffer AGREE on every run, and
/// deleting the branch changes no simulation result. (The wasm backend is not so
/// lucky: its `reset` does not clear the snapshot regions, which is why the
/// `select` there IS observable and is pinned by
/// `wasmgen::module_tests::compile_simulation_repeated_run_resets_previous_fallback_for_an_array_view`.)
/// Keeping the branch anyway is what makes the two backends state the same rule
/// rather than one relying on an initialization the other does not perform.
///
/// Rows are the full cross of the enumeration -- 4 `ViewStorage` arms x
/// `use_prev_fallback` x `StepPart::Initials`-or-not -- rather than only the
/// cells that vary, so an arm that starts reading a flag it should ignore is
/// caught too. `Curr` and `Temp` are inert on both factors; `Prev` varies with
/// the flag alone; `Initial` with the phase alone.
#[cfg(test)]
mod chunk_regions_tests {
    use super::*;
    use smallvec::smallvec;

    /// Each region carries a distinct value at slot 0, so the value read
    /// identifies WHICH region was selected -- a resolver that returned the
    /// right slice for the wrong reason cannot pass.
    const CURR: f64 = 1.0;
    const PREV: f64 = 2.0;
    const INITIAL: f64 = 3.0;
    const TEMP: f64 = 4.0;

    fn context() -> ByteCodeContext {
        let mut ctx = ByteCodeContext::default();
        ctx.set_temp_info(vec![0], 1);
        ctx
    }

    fn view(storage: ViewStorage) -> RuntimeView {
        let mut v = RuntimeView::for_var(0, smallvec![1], smallvec![0]);
        v.storage = storage;
        v
    }

    /// The value `backing` selects for `storage` under the given run state, or
    /// `None` when it reports "this view reads the PREVIOUS fallback".
    fn resolve(storage: ViewStorage, use_prev_fallback: bool, part: StepPart) -> Option<f64> {
        let curr = [CURR];
        let prev = [PREV];
        let initial = [INITIAL];
        let temp = [TEMP];
        let regions = ChunkRegions {
            curr: &curr,
            prev: &prev,
            initial: &initial,
            use_prev_fallback,
            part,
        };
        regions
            .backing(&view(storage), &temp, &context())
            .map(|(data, base)| data[base])
    }

    #[test]
    fn backing_routes_every_storage_arm_under_every_run_state() {
        for &part in &[StepPart::Initials, StepPart::Flows, StepPart::Stocks] {
            for &fallback in &[true, false] {
                let ctx = format!("part={part:?} use_prev_fallback={fallback}");

                // Curr and Temp read their own region unconditionally.
                assert_eq!(
                    resolve(ViewStorage::Curr, fallback, part),
                    Some(CURR),
                    "Curr must be inert on both factors ({ctx})"
                );
                assert_eq!(
                    resolve(ViewStorage::Temp, fallback, part),
                    Some(TEMP),
                    "Temp must be inert on both factors ({ctx})"
                );

                // Prev: the fallback while no snapshot exists, the snapshot
                // after. This mirrors `Opcode::LoadPrev` exactly, and the
                // fallback an ARRAY-valued PREVIOUS may carry is always 0
                // (`codegen::is_default_previous_fallback`), which is what
                // `None` means to every caller.
                assert_eq!(
                    resolve(ViewStorage::Prev, fallback, part),
                    if fallback { None } else { Some(PREV) },
                    "Prev must follow use_prev_fallback and nothing else ({ctx})"
                );

                // Initial: `curr` during the initials phase (the snapshot has
                // not been captured yet -- `curr` IS the initial value being
                // computed), the snapshot otherwise. Mirrors
                // `Opcode::LoadInitial`.
                assert_eq!(
                    resolve(ViewStorage::Initial, fallback, part),
                    Some(if part == StepPart::Initials {
                        CURR
                    } else {
                        INITIAL
                    }),
                    "Initial must follow the phase and nothing else ({ctx})"
                );
            }
        }
    }
}

/// Tests for [`Vm::reduce_view`]'s slicing, over views built by hand.
///
/// NOT covered here, and currently covered NOWHERE: the documented empty-view
/// contract that ArrayMax/Min/Mean/Stddev push NaN while ArraySum yields 0.0.
/// That asymmetry lives in the OPCODE arms, not in `reduce_view` -- all four
/// `view.size() == 0` guards can be deleted with the suite green. Pinning it
/// needs an opcode-level fixture, and a zero-element dimension cannot be
/// built through the compilation pipeline, so it needs hand-assembled
/// bytecode.
#[cfg(test)]
mod reduce_view_tests {
    use super::*;

    fn empty_context() -> ByteCodeContext {
        ByteCodeContext {
            graphical_functions: vec![],
            modules: vec![],
            arrays: vec![],
            dimensions: vec![],
            subdim_relations: vec![],
            names: vec![],
            static_views: vec![],
            temp_offsets: vec![],
            temp_total_size: 0,
            dim_lists: vec![],
        }
    }

    #[test]
    fn reduce_view_returns_nan_for_invalid_view() {
        let view = RuntimeView::invalid();
        let curr: [f64; 0] = [];
        let temp: [f64; 0] = [];
        let ctx = empty_context();
        let result = Vm::reduce_view(
            &temp,
            &view,
            ChunkRegions::curr_only(&curr),
            &ctx,
            |acc, v| acc + v,
            0.0,
        );
        assert!(result.is_nan());
    }

    // -- reduce over an offset row-slice: a dense linear run that is NOT
    // `is_contiguous()` (offset != 0). Pins the dense-linear fast path to the
    // same elements (and the same left-to-right fold order) as the general
    // index-decompose path. --
    #[test]
    fn reduce_view_offset_row_slice() {
        // A 3x4 array at base 2 of curr; view = arr[1, *] (flat 4..8).
        let curr: Vec<f64> = (0..16).map(|i| i as f64).collect();
        let temp: [f64; 0] = [];
        let ctx = empty_context();
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[u16; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(2, dims, dim_ids);
        view.apply_single_subscript(0, 1);
        assert!(!view.is_contiguous(), "offset slice must not be contiguous");

        // elements are curr[2 + 4 .. 2 + 8] = [6, 7, 8, 9]
        let sum = Vm::reduce_view(
            &temp,
            &view,
            ChunkRegions::curr_only(&curr),
            &ctx,
            |acc, v| acc + v,
            0.0,
        );
        assert_eq!(sum, 30.0);
        let max = Vm::reduce_view(
            &temp,
            &view,
            ChunkRegions::curr_only(&curr),
            &ctx,
            |acc, v| if v > acc { v } else { acc },
            f64::NEG_INFINITY,
        );
        assert_eq!(max, 9.0);
    }

    // -- reduce over an inner-dimension range slice: NOT a dense linear run,
    // must keep taking the general per-element path. --
    #[test]
    fn reduce_view_inner_range_slice() {
        // A 3x4 array at base 0; view = arr[*, 1:3) -> columns 1,2 of each row.
        let curr: Vec<f64> = (0..12).map(|i| i as f64).collect();
        let temp: [f64; 0] = [];
        let ctx = empty_context();
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[u16; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.apply_range(1, 1, 3);
        assert_eq!(view.dense_linear_start(), None);

        // elements: 1,2, 5,6, 9,10
        let sum = Vm::reduce_view(
            &temp,
            &view,
            ChunkRegions::curr_only(&curr),
            &ctx,
            |acc, v| acc + v,
            0.0,
        );
        assert_eq!(sum, 33.0);
    }
}
