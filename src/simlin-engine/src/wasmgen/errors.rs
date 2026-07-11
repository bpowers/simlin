// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: the emitters append wasm instruction sequences, and the
// host-side decoder is a pure function of the reported error state plus the
// compile-time plan list. No I/O; the tests execute the emitted modules under
// the DLR-FT interpreter and diff the reconstructed message against the VM's.

//! The emitted module's runtime error channel (GH #921).
//!
//! ## Why a channel exists at all
//!
//! Every construct the wasm backend lowered before the special-stock passes is
//! *total*: division by zero yields an infinity, a bad table index yields NaN,
//! an out-of-range subscript yields NaN. So the blob had nothing to report and
//! `run`/`run_to`/`run_initials` had no early-return path. The conveyor belt
//! pass (GH #922) breaks that: `conveyor_compile::check_slat_bound` rejects a
//! latched transit time whose slat count exceeds [`crate::conveyor::slat_bound`],
//! both at belt init (`init_belts`) and at a mid-run `<sample>` re-latch
//! (`run_phase_a`), and the bytecode VM turns that into a hard `Err` out of
//! `run_initials` / `run_to`.
//!
//! This module is the blob's equivalent. It is deliberately narrow: a code, a
//! belt index, and an exported getter. The blob builds no strings -- it has no
//! allocator, no formatter, and no `ConveyorPlan` in linear memory -- so the
//! HOST rebuilds the VM's exact message from the plan list it already holds (see
//! [`reconstruct_error`]).
//!
//! ## The wire format
//!
//! Two internal mutable i32 globals hold the state:
//!
//! * [`G_ERR_CODE`] -- `0` when no error has been raised, else the offending
//!   [`ErrorCode`]'s discriminant (`ErrorCode as i32`). `ErrorCode::NoError` is
//!   discriminant 0 and is documented as never produced, so `0` is unambiguous.
//! * [`G_ERR_BELT`] -- the index into the pass's plan list of the belt that
//!   raised. Meaningless when the code is 0.
//!
//! A single new export, `get_error() -> i64`, returns both atomically: the low
//! 32 bits are the code, the high 32 bits the belt index. One export rather than
//! two exported globals so a host can never pair a code from one run with a belt
//! index from another; [`decode_error_word`] is the canonical unpacker and
//! [`emit_get_error`] the canonical packer, so the two cannot drift.
//!
//! ## The unwind contract (what GH #922 must honor)
//!
//! See [`ErrorScope`]. In short: a driver wraps an error-capable pass body in a
//! `block`, the pass raises with [`ErrorScope::raise`] (which sets the two
//! globals and `br`s to that block's end), and the *driver* decides what to do
//! next -- `run_to`'s step site returns from the whole function without saving a
//! row, while its mid-run preview site restores `curr`, clears the channel, and
//! still runs its epilogue.
//!
//! ## Sticky until `reset`
//!
//! Once the channel is set, `run_initials` and `run_to` are no-ops until `reset`
//! clears it. This is a deliberate divergence from the VM, which re-attempts the
//! failing step on the next `run_to` call; see [`emit_return_if_error`].

use wasm_encoder::{BlockType, Function, Instruction as I};

use crate::common::ErrorCode;
use crate::conveyor_compile::ConveyorPlan;

/// Index of the error-code global. It follows the seven globals `module.rs`
/// reserves (three immutable geometry globals, `use_prev_fallback`, and the
/// three-word step cursor).
///
/// Unlike `passes::G_HEAP` (which follows at index 9), both error globals are
/// emitted for EVERY module, whether or not it carries a pass that can raise.
/// The alternative -- conditional globals plus a conditional `get_error` export
/// -- would force every host to feature-detect the channel before checking it,
/// to learn something it already knows statically. Two i32 globals and a
/// four-instruction getter are a negligible price for an unconditional ABI.
pub(super) const G_ERR_CODE: u32 = 7;

/// Index of the error-belt global (the plan-list index of the belt that raised).
/// See [`G_ERR_CODE`].
pub(super) const G_ERR_BELT: u32 = 8;

/// Emit the module's sole error export, `get_error() -> i64`.
///
/// Returns `(belt << 32) | code`, both halves zero-extended from their i32
/// globals. Zero means "no error" (`ErrorCode::NoError`'s discriminant is 0 and
/// the enum documents it as never produced). [`decode_error_word`] is the
/// matching unpacker.
pub(super) fn emit_get_error() -> Function {
    let mut f = Function::new([]);
    // Zero-extend rather than sign-extend: the belt index is a plan-list index
    // (never negative) and a sign-extended code would smear 1-bits across the
    // belt half. Nothing writes a negative value into either global, so the two
    // agree today -- but zero-extension is what makes the packing total.
    f.instruction(&I::GlobalGet(G_ERR_BELT));
    f.instruction(&I::I64ExtendI32U);
    f.instruction(&I::I64Const(32));
    f.instruction(&I::I64Shl);
    f.instruction(&I::GlobalGet(G_ERR_CODE));
    f.instruction(&I::I64ExtendI32U);
    f.instruction(&I::I64Or);
    f.instruction(&I::End);
    f
}

/// Emit `G_ERR_CODE = 0; G_ERR_BELT = 0` -- clear the channel.
///
/// Two call sites: `reset` (the host's only way to recover from a raised error)
/// and `run_to`'s mid-run preview, which swallows a preview-only failure exactly
/// as `vm.rs`'s cloned-side-table preview does.
pub(super) fn emit_clear(f: &mut Function) {
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_ERR_CODE));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_ERR_BELT));
}

/// Emit `if G_ERR_CODE != 0 { return }` -- the driver-side guard.
///
/// Three call sites, all in `module.rs`:
///
/// 1. Top of `run_initials`, after the `G_DID_INITIALS` idempotency guard.
/// 2. In `run_to`, immediately after its `call run_initials`.
/// 3. In the Euler step, immediately after the between-Flows-and-Stocks pass
///    block -- so a raising step returns *before* the Stocks phase, the
///    `prev_values` snapshot, and the save/advance tail. The failing step's row
///    is therefore never written, matching `vm.rs`'s `return Err(...)` between
///    `run_coupled_passes` and the Stocks `eval`.
///
/// Sites 1 and 2 together give the channel **sticky** semantics: once set, every
/// subsequent `run_initials`/`run_to` is a no-op until `reset` clears it. That is
/// a deliberate divergence from the VM, which re-attempts the failing step on the
/// next `run_to`. Two reasons to diverge:
///
/// * The VM's re-attempt is not sound to begin with. `run_phase_a` errors partway
///   through the belt list, having already advanced (and written driven rates
///   for) belts `0..i`; resuming re-runs Phase A for all of them, double-advancing
///   the ones that succeeded. Refusing to continue is the conservative reading.
/// * A wasm export cannot return a `Result`, so a host that forgets to poll
///   `get_error` between calls would otherwise silently accumulate rows computed
///   from a half-advanced side table. Fail-closed is the only safe default when
///   the failure cannot be forced on the caller.
///
/// `reset` (and therefore `run`, which is `reset; run_to(stop)`) clears the
/// channel, so a fresh run is never blocked by a stale error.
pub(super) fn emit_return_if_error(f: &mut Function) {
    f.instruction(&I::GlobalGet(G_ERR_CODE));
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::Return);
    f.instruction(&I::End);
}

/// A capability to raise a runtime error, carrying the wasm label depth between
/// the current emit point and the enclosing **pass block** -- the `block` a driver
/// wraps every error-capable side-table pass body in.
///
/// # The unwind contract
///
/// A pass emitter raises a runtime error with [`raise`], which stores the code
/// and belt index into [`G_ERR_CODE`]/[`G_ERR_BELT`] and then branches to the end
/// of the pass block. Nothing after the raise executes, so the emitted pass
/// abandons the remaining belts exactly as the VM's `?` abandons the remaining
/// iterations of `init_belts` / `run_phase_a`.
///
/// **`br`, not `return`.** The pass body is emitted at TWO sites and they must
/// react differently: the real between-Flows-and-Stocks step abandons the whole
/// `run_to`, while the mid-run PREVIEW must restore `curr`, clear the channel, and
/// still run its epilogue (restore the saved descriptors, rewind the bump
/// pointer). A `return` from inside the preview would skip that epilogue and leak
/// the cloned side table. Branching to the block's end lets the *driver* decide,
/// and lets one pass body serve both sites unchanged.
///
/// # A scope cannot be forged
///
/// The only constructor is [`open_pass_block`], which emits the `block` in the
/// same breath. That is not stylistic. A `br` with no enclosing pass block still
/// *validates*: the label resolves to whatever construct happens to enclose the
/// emit point, which at the step site is `run_to`'s `loop`. The blob would then
/// spin forever instead of trapping -- an infinite loop is the worst possible
/// failure mode for a codegen bug, because it produces no diagnostic at all. So a
/// pass body receives `Option<ErrorScope>`: `None` at a site whose driver did not
/// open a block, and a pass that must raise turns that into a loud emit-time
/// panic. There is no path from "the driver forgot to set `can_error`" to a
/// hanging blob.
///
/// Because `br` takes a *relative* label depth, an emitter must keep the scope in
/// sync with its own nesting: [`open_pass_block`] hands back depth 0 (directly
/// inside the pass block), and an emitter that opens a `block`/`loop`/`if` passes
/// an [`entered`] COPY down into it. There is deliberately no `exit`: the scope is
/// `Copy` and restoring it is lexical, so an emitter cannot leave the depth
/// desynchronized from its own `end`s.
///
/// ```ignore
/// let scope = errors::open_pass_block(f);  // emits `block`
/// f.instruction(&I::If(BlockType::Empty));
/// scope.entered().raise(f, ErrorCode::ConveyorTransitTooLong, belt); // br 1
/// f.instruction(&I::End);
/// scope.raise(f, ..);                                                // br 0
/// errors::close_pass_block(f, scope);      // emits `end`
/// ```
///
/// Raising from inside a *helper function* is not supported: `br` cannot cross a
/// call boundary. A helper that needs to fail must return a flag its (inline)
/// caller tests, or -- as `passes::emit_alloc` does for a failed `memory.grow` --
/// trap.
///
/// [`raise`]: ErrorScope::raise
/// [`entered`]: ErrorScope::entered
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ErrorScope {
    depth: u32,
}

/// Open a pass block and mint the [`ErrorScope`] that unwinds to its end.
///
/// Pair with [`close_pass_block`]. This is the SOLE constructor of an
/// `ErrorScope`, so the capability to `br` out of a pass block cannot exist
/// unless the block does; see the type's docs for why that matters.
pub(super) fn open_pass_block(f: &mut Function) -> ErrorScope {
    f.instruction(&I::Block(BlockType::Empty));
    ErrorScope { depth: 0 }
}

/// Close the `block` [`open_pass_block`] opened.
///
/// Takes the scope back to make the pairing legible at the call site, and to
/// assert the caller closed every construct it opened after minting it: a scope
/// that is still `entered` here means an `end` went missing, and the pass block's
/// label is not where the emitter thinks it is.
pub(super) fn close_pass_block(f: &mut Function, scope: ErrorScope) {
    debug_assert_eq!(
        scope.depth, 0,
        "pass block closed from {} labels deep: an `end` is missing",
        scope.depth
    );
    f.instruction(&I::End);
}

impl ErrorScope {
    /// The scope one label deeper: what an emitter passes into the body of a
    /// `block`/`loop`/`if` it just opened.
    //
    // No production caller yet: the conveyor belt pass (GH #922) is the only pass
    // that raises, and it is not lowered. This and `raise` are the contract that
    // pass is written against, so they belong with the channel rather than being
    // invented alongside the belt. The test fault injector below exercises both.
    // The allow is scoped to non-test builds so an item that goes unused *there
    // too* still warns.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn entered(self) -> Self {
        ErrorScope {
            depth: self.depth + 1,
        }
    }

    /// Emit `G_ERR_CODE = code; G_ERR_BELT = belt; br <pass block>`.
    ///
    /// `belt` is an index into the pass's plan list, which is what
    /// [`reconstruct_error`] uses to recover the belt's name and its transit-time
    /// slot. Both operands are compile-time constants (a pass is unrolled per
    /// plan), so this is four instructions with no runtime arithmetic.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn raise(&self, f: &mut Function, code: ErrorCode, belt: usize) {
        f.instruction(&I::I32Const(code as i32));
        f.instruction(&I::GlobalSet(G_ERR_CODE));
        f.instruction(&I::I32Const(belt as i32));
        f.instruction(&I::GlobalSet(G_ERR_BELT));
        f.instruction(&I::Br(self.depth));
    }
}

/// The panic a pass emitter raises when handed `None` where it needs a scope --
/// i.e. when the driver did not open a pass block because the module was built
/// with `pass_can_error == false`.
///
/// A loud emit-time panic is the whole point: the alternative (emitting the `br`
/// anyway) validates and hangs. See [`ErrorScope`].
//
// Non-test callers arrive with GH #922's belt pass; the fault injector below is
// today's only one. Scoped to non-test builds so it still warns if it goes unused
// there too.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn expect_scope(scope: Option<ErrorScope>) -> ErrorScope {
    scope.expect(
        "a pass that raises was emitted at a site with no unwind block: \
         `module::pass_can_error` must return true for every module whose passes \
         call `ErrorScope::raise`, or the `br` targets an enclosing construct \
         (at the step site, `run_to`'s loop) and the blob spins forever",
    )
}

// ── the host side ────────────────────────────────────────────────────────────

/// The runtime error state a blob reported through `get_error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobError {
    /// The offending [`ErrorCode`]'s discriminant (`ErrorCode as i32`), never 0.
    pub code: i32,
    /// Index into the conveyor plan list of the belt that raised.
    pub belt: usize,
}

/// Unpack the `i64` a blob's `get_error` export returns.
///
/// `None` means the run completed without a runtime error. The check is on the
/// CODE half, not on the whole word: `reset` clears both halves, but a host that
/// reads the word after a cleared preview failure would otherwise see a stale
/// belt index alone as an error.
pub fn decode_error_word(word: i64) -> Option<BlobError> {
    let code = (word & 0xffff_ffff) as u32 as i32;
    if code == 0 {
        return None;
    }
    Some(BlobError {
        code,
        belt: ((word >> 32) as u32) as usize,
    })
}

/// Rebuild the exact `(ErrorCode, String)` pair the bytecode VM raises for the
/// runtime error a wasm blob reported, so the two backends are indistinguishable
/// to a caller.
///
/// `word` is the raw `get_error()` result; `plans` the conveyor plan list the
/// blob was compiled from (`queue_compile::compile_sim`'s `SimBuild`); `dt` the
/// simulation timestep; and `read_curr_slot` a reader for slot `off` of the
/// blob's live `curr` chunk (byte address `off * 8`, since `curr`'s base is 0).
///
/// Returns `None` when no error was raised.
///
/// # Why the transit time comes from linear memory
///
/// Both conveyor runtime errors print the belt's transit time. The blob does not
/// stash it: the value is already sitting in `curr[plan.len_off]`, no pass ever
/// writes that slot, and both raise sites abandon the step before anything else
/// touches `curr`. Re-reading it is therefore exact, and it keeps the channel to
/// two words.
///
/// That the slot is POPULATED at both sites is a property of the drivers, not an
/// accident. `len_off` names a synthesized belt-parameter aux, which nothing
/// depends on and which is therefore absent from the initials runlist -- it is
/// written only by the Flows phase. At the mid-run raise site Flows has just run.
/// At the `run_initials` raise site it has not, so the driver evaluates Flows
/// before entering the init hook (`module::Passes::needs_flows_before_init`,
/// mirroring `vm.rs:1508-1514`, which inserts the same evaluation for the same
/// reason). Without it a belt would read a transit of 0 and raise a spurious
/// `ConveyorTransitNotPositive`, and this reconstruction would faithfully print
/// the lie.
///
/// The `ConveyorTransitTooLong` message also prints `slat_count(transit, dt)`.
/// The VM's mid-run site prints the CLAMPED transit (`clamp_transit(v, dt) =
/// max(v, dt)` for finite `v`) while its init site prints the raw slot value; the
/// two coincide wherever the error can fire. For any `slat_bound() >= 1` the
/// error needs `slat_count(clamped) >= 2`, hence `clamped >= 1.5*dt`, hence
/// `clamped != dt`, hence `clamped == v`. So reading the raw slot reproduces both
/// sites' text.
///
/// # Byte-identity
///
/// The message strings are not duplicated here: this delegates to
/// [`crate::conveyor_compile::transit_too_long_error`] and
/// [`crate::conveyor_compile::transit_not_positive_error`], the same constructors
/// the VM's `init_belts` / `run_phase_a` call. Those read
/// [`crate::conveyor::slat_bound`], so a test holding a `SlatBoundGuard` sees the
/// same bound the emitter baked in.
///
/// An unrecognized code, or a belt index outside `plans`, yields a loud
/// `ErrorCode::Generic` describing the raw state rather than a panic or a silent
/// `None`: a blob and a host that disagree about the channel is a bug worth
/// surfacing, not one worth crashing on.
pub fn reconstruct_error(
    word: i64,
    plans: &[ConveyorPlan],
    dt: f64,
    read_curr_slot: impl Fn(usize) -> f64,
) -> Option<(ErrorCode, String)> {
    let BlobError { code, belt } = decode_error_word(word)?;

    let Some(plan) = plans.get(belt) else {
        return Some((
            ErrorCode::Generic,
            format!(
                "wasmgen: blob reported runtime error code {code} for belt {belt}, \
                 but the model has only {} conveyor plan(s)",
                plans.len()
            ),
        ));
    };
    let transit = read_curr_slot(plan.len_off);

    if code == ErrorCode::ConveyorTransitTooLong as i32 {
        Some(crate::conveyor_compile::transit_too_long_error(
            &plan.name, transit, dt,
        ))
    } else if code == ErrorCode::ConveyorTransitNotPositive as i32 {
        Some(crate::conveyor_compile::transit_not_positive_error(
            &plan.name, transit,
        ))
    } else {
        Some((
            ErrorCode::Generic,
            format!(
                "wasmgen: blob reported unknown runtime error code {code} \
                 (conveyor '{}', plan index {belt})",
                plan.name
            ),
        ))
    }
}

// ── test-only fault injection ────────────────────────────────────────────────
//
// The conveyor belt pass (GH #922) does not exist yet, so no production model
// can raise. To prove the mechanism -- the unwind, the driver guards, the
// no-row-saved semantics, the preview's swallow-and-restore -- the tests splice
// a synthetic one-instruction "pass" into the SAME hook points the belt pass will
// occupy.
//
// Outside a test build [`FaultInjection`] is an UNINHABITED enum. That keeps
// `module.rs` free of `#[cfg(test)]`: it stores an ordinary
// `Option<FaultInjection>`, which shipped code can only ever construct as `None`,
// and every `if let Some(fault) = ..` arm is statically dead. The alternative --
// cfg-gating the field and each emit site -- costs more conditional compilation
// than the machinery is worth, and makes the two builds structurally different.

/// A synthetic side-table pass that raises a chosen error at a chosen hook
/// (test-only; see the module comment above).
///
/// Uninhabited outside a test build, so `Option<FaultInjection>` is always `None`
/// and none of the emit sites below can run. The methods still exist so the call
/// sites type-check unconditionally; each discharges the impossible `&self` with
/// `match *self {}`.
#[cfg(not(test))]
pub(super) enum FaultInjection {}

#[cfg(not(test))]
impl FaultInjection {
    pub(super) fn emit_initials(&self, _f: &mut Function, _scope: Option<ErrorScope>) {
        match *self {}
    }
    pub(super) fn emit_step(&self, _f: &mut Function, _scope: Option<ErrorScope>) {
        match *self {}
    }
    pub(super) fn emit_publish(&self, _f: &mut Function) {
        match *self {}
    }
    pub(super) fn needs_flows_before_init(&self) -> bool {
        match *self {}
    }
}

/// Where a [`FaultInjection`] raises.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) enum FaultSite {
    /// Unconditionally, from `run_initials`'s side-table init hook -- the wasm
    /// twin of `init_belts` returning `Err` before the VM sets `did_initials`.
    Initials,
    /// From the between-Flows-and-Stocks step hook, once `curr[TIME] >= at_time`.
    /// The mid-run preview runs the same body at the resting time, so a fault
    /// whose `at_time` lies just past a `run_to` target fires in the preview
    /// only -- which is how the swallow-and-restore path is exercised.
    Step { at_time: f64 },
}

/// A synthetic side-table pass that raises a chosen error at a chosen hook.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(super) struct FaultInjection {
    pub code: ErrorCode,
    pub belt: usize,
    pub site: FaultSite,
    /// A slab slot the pass stamps with the given value immediately BEFORE
    /// raising, standing in for the half-written pass output (driven flow rates,
    /// published container values) the VM's preview snapshot/restore exists to
    /// undo. `None` writes nothing.
    pub marker: Option<(usize, f64)>,
    /// A slab slot stamped at the pass block's TOP LEVEL, after the raise site --
    /// standing in for a later belt's contribution to the pass. A correct
    /// [`ErrorScope::raise`] branches clean out of the pass block, so this must
    /// never execute once the fault fires: that is the "abandon the remaining
    /// belts" contract, and the only observable that pins the `br`'s label depth
    /// (a `br` one label short would merely leave the conditional and fall through
    /// to here). It DOES execute on a run where the fault never fires, so tests
    /// that arm the fault out of range leave it `None`.
    pub later_belt_marker: Option<(usize, f64)>,
    /// A slab slot stamped from the step-start CONTAINER PUBLISH hook, standing in
    /// for `conveyor_compile::publish_container_values` reading the side table.
    ///
    /// Publishing from a side table whose init raised would read never-initialized
    /// belt state; so would the Flows phase that follows it. The two driver guards
    /// -- `run_initials`' post-init-block guard and `run_to`'s post-`run_initials`
    /// guard -- exist precisely to stop that, and a test that fails the init and
    /// finds this slot untouched has pinned both. Stamped on EVERY publish, so only
    /// an `Initials`-site fault should set it.
    pub publish_marker: Option<(usize, f64)>,
    /// Whether this pass needs a Flows evaluation before its init hook, standing in
    /// for the conveyor pass's dependence on the synthesized belt-parameter auxes
    /// (see `module::Passes::needs_flows_before_init`).
    pub needs_flows: bool,
    /// `(dst, src)`: at the init hook, before raising, copy `curr[src]` into
    /// `curr[dst]`. The observable for [`Self::needs_flows`]: point `src` at a slot
    /// only the Flows phase writes and `dst` at an inert one, and the copied value
    /// says whether the driver ran Flows before handing control to the init hook.
    pub init_probe: Option<(usize, usize)>,
}

#[cfg(test)]
impl FaultInjection {
    /// Byte address of `curr[TIME]`: slot 0 of the `curr` chunk, whose base is 0.
    const CURR_TIME_ADDR: u64 = 0;

    /// Whether the driver must evaluate Flows before the init hook.
    pub(super) fn needs_flows_before_init(&self) -> bool {
        self.needs_flows
    }

    /// The `run_initials` hook body. Emitted inside the pass block, so a raise
    /// branches straight to its end and `run_initials` returns without arming the
    /// step cursor or setting `G_DID_INITIALS`.
    pub(super) fn emit_initials(&self, f: &mut Function, scope: Option<ErrorScope>) {
        if !matches!(self.site, FaultSite::Initials) {
            return;
        }
        self.emit_probe(f);
        self.emit_marker(f, self.marker);
        expect_scope(scope).raise(f, self.code, self.belt);
        self.emit_marker(f, self.later_belt_marker);
    }

    /// The step-start container-publish hook (a hook point distinct from the pass
    /// proper). Never raises: a real `publish_container_values` cannot fail.
    pub(super) fn emit_publish(&self, f: &mut Function) {
        self.emit_marker(f, self.publish_marker);
    }

    /// The step hook body (also the preview body). Emitted inside the pass block.
    ///
    /// `later_belt_marker` gets its OWN copy of the firing condition rather than
    /// sitting bare after the `if`: a bare store would run on every step the fault
    /// does not fire, polluting the very slot the test reads. Guarded, it is
    /// reachable on exactly one execution -- the firing step, and only if the
    /// raise's `br` failed to leave the pass block.
    pub(super) fn emit_step(&self, f: &mut Function, scope: Option<ErrorScope>) {
        let FaultSite::Step { at_time } = self.site else {
            return;
        };
        let scope = expect_scope(scope);
        Self::emit_fires(f, at_time);
        f.instruction(&I::If(BlockType::Empty));
        // Inside the `if`, the pass block is one label further out.
        let inner = scope.entered();
        self.emit_marker(f, self.marker);
        inner.raise(f, self.code, self.belt);
        f.instruction(&I::End);

        Self::emit_fires(f, at_time);
        f.instruction(&I::If(BlockType::Empty));
        self.emit_marker(f, self.later_belt_marker);
        f.instruction(&I::End);
    }

    /// Push `curr[TIME] >= at_time` as an i32 condition.
    fn emit_fires(f: &mut Function, at_time: f64) {
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(super::lower::memarg(Self::CURR_TIME_ADDR)));
        f.instruction(&super::lower::f64_const(at_time));
        f.instruction(&I::F64Ge);
    }

    /// `curr[dst] = curr[src]`, the [`FaultInjection::init_probe`] copy.
    fn emit_probe(&self, f: &mut Function) {
        let Some((dst, src)) = self.init_probe else {
            return;
        };
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(super::lower::memarg(src as u64 * 8)));
        f.instruction(&I::F64Store(super::lower::memarg(dst as u64 * 8)));
    }

    fn emit_marker(&self, f: &mut Function, marker: Option<(usize, f64)>) {
        let Some((off, value)) = marker else {
            return;
        };
        f.instruction(&I::I32Const(0));
        f.instruction(&super::lower::f64_const(value));
        f.instruction(&I::F64Store(super::lower::memarg(off as u64 * 8)));
    }
}

#[cfg(test)]
#[path = "errors_tests.rs"]
mod tests;
