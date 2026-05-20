// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Bytecode composition profiling for a compiled simulation.
//!
//! A diagnostics-only sibling of `vm.rs` (kept here purely for the per-file line
//! cap): `CompiledSimulation::bytecode_profile` answers "how big and what shape
//! is the compiled bytecode?" for the `clearn_profile` example and similar
//! analysis, without exposing the private `Opcode` type.

use std::collections::BTreeMap;

use crate::bytecode::ByteCode;
use crate::vm::CompiledSimulation;

impl CompiledSimulation {
    /// Walk every compiled module's bytecode and tables to produce an aggregate
    /// composition profile.
    pub fn bytecode_profile(&self) -> BytecodeProfile {
        let mut p = BytecodeProfile {
            n_modules: self.modules.len(),
            n_slots_root: self.n_slots(),
            ..Default::default()
        };

        let mut tally = |bc: &ByteCode, hist: &mut BTreeMap<&'static str, usize>| {
            p.total_literals += bc.literals.len();
            for op in bc.code.iter() {
                *hist.entry(op.name()).or_insert(0) += 1;
            }
            bc.code.len()
        };

        // Histogram-only tally with no `total_literals` side effect. The fused
        // streams below are temporary clones of the same module bytecode and
        // share its literal table (fusion rewrites opcodes, never the literal
        // pool), which `tally` already counts on the real bytecode -- counting
        // it again would double `total_literals`.
        let tally_hist = |bc: &ByteCode, hist: &mut BTreeMap<&'static str, usize>| {
            for op in bc.code.iter() {
                *hist.entry(op.name()).or_insert(0) += 1;
            }
        };

        for module in self.modules.values() {
            p.flow_opcodes += tally(&module.compiled_flows, &mut p.histogram);
            // Measure the post-fusion size by running the *actual* fusion pass on
            // a clone (the pass is what the Vm applies at construction), rather
            // than a separate estimate that could drift from the real pass. The
            // fused histogram tallies the executed flow+stock stream as the Vm
            // sees it, so the per-fused-opcode counts (e.g. how many BinGlobal*
            // / BinConstConst sites fired) are observable.
            let mut fused = module.compiled_flows.as_ref().clone();
            fused.fuse_three_address();
            p.flow_opcodes_after_fusion += fused.code.len();
            tally_hist(&fused, &mut p.fused_histogram);
            let mut fused_stocks = module.compiled_stocks.as_ref().clone();
            fused_stocks.fuse_three_address();
            tally_hist(&fused_stocks, &mut p.fused_histogram);
            p.stock_opcodes += tally(&module.compiled_stocks, &mut p.histogram);
            for ci in module.compiled_initials.iter() {
                p.n_initials += 1;
                p.initial_opcodes += tally(&ci.bytecode, &mut p.histogram);
            }

            let ctx = &module.context;
            p.graphical_functions += ctx.graphical_functions.len();
            p.graphical_function_points += ctx
                .graphical_functions
                .iter()
                .map(|gf| gf.len())
                .sum::<usize>();
            p.temp_storage_slots += ctx.temp_total_size;
            p.dimensions += ctx.dimensions.len();
            p.static_views += ctx.static_views.len();
            p.dim_lists += ctx.dim_lists.len();
            p.names += ctx.names.len();
        }

        p.total_opcodes = p.flow_opcodes + p.stock_opcodes + p.initial_opcodes;
        p
    }
}

/// Aggregate composition of a compiled simulation's bytecode and side tables.
/// Produced by [`CompiledSimulation::bytecode_profile`]. `histogram` maps each
/// opcode variant name to its occurrence count across all modules and phases.
#[derive(Default, Clone)]
pub struct BytecodeProfile {
    pub n_modules: usize,
    pub n_slots_root: usize,
    pub total_opcodes: usize,
    pub flow_opcodes: usize,
    /// Estimated flow opcode count after a 3-address fusion pass (R2 sizing).
    pub flow_opcodes_after_fusion: usize,
    pub stock_opcodes: usize,
    pub initial_opcodes: usize,
    pub n_initials: usize,
    pub total_literals: usize,
    pub graphical_functions: usize,
    pub graphical_function_points: usize,
    pub temp_storage_slots: usize,
    pub dimensions: usize,
    pub static_views: usize,
    pub dim_lists: usize,
    pub names: usize,
    pub histogram: BTreeMap<&'static str, usize>,
    /// Opcode histogram of the *post-fusion* flow+stock stream (the program the
    /// Vm actually dispatches), so the count of each fused superinstruction
    /// (BinVarVar, BinGlobal*, BinConstConst, ...) is directly observable.
    pub fused_histogram: BTreeMap<&'static str, usize>,
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestProject;

    /// Regression guard: `bytecode_profile` must count each compiled phase's
    /// literal table exactly once. The fused-stream histogram is tallied from
    /// temporary clones that share the real bytecode's literal pool, so it must
    /// not contribute to `total_literals` -- otherwise every module's flow and
    /// stock literals are counted twice.
    #[test]
    fn total_literals_excludes_fused_clone_tally() {
        let compiled = TestProject::new("profile_literals")
            .aux("rate", "0.5", None)
            .stock("s", "100", &["inflow"], &[], None)
            .flow("inflow", "s * rate + 3", None)
            .compile_incremental()
            .unwrap();

        // Ground truth: each phase's literal table, counted once.
        let mut expected = 0usize;
        for m in compiled.modules.values() {
            expected += m.compiled_flows.literals.len();
            expected += m.compiled_stocks.literals.len();
            for ci in m.compiled_initials.iter() {
                expected += ci.bytecode.literals.len();
            }
        }
        assert!(
            expected > 0,
            "test model should produce literals (else vacuous)"
        );

        assert_eq!(
            compiled.bytecode_profile().total_literals,
            expected,
            "total_literals must count each phase's literal table once; the \
             fused-clone tally must not re-count the shared literal pool"
        );
    }
}
