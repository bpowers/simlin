// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// deny (not forbid) because vm.rs Stack needs a targeted #[allow(unsafe_code)]
// for unchecked array access in the hot dispatch loop. Rust's forbid() cannot
// be overridden by inner #[allow] attributes (even in submodules), so deny()
// is the strongest level that still permits a single opt-in. The unsafe stack
// access is proven safe by compiler::symbolic::resolve_bytecode(), which
// statically validates that compiled bytecode cannot exceed STACK_CAPACITY.
#![deny(unsafe_code)]

// Re-exported so downstream crates (libsimlin, integration tests) can name the
// `IndexMap` type of `db::LtmVariablesResult.loop_partitions` without taking a
// direct `indexmap` dependency. The emission-order iteration is load-bearing
// for the post-sim rel-loop-score denominator (GH #468), so callers thread the
// `IndexMap` through rather than collapsing it to a `HashMap`.
pub use indexmap;

#[cfg(feature = "ai_info")]
pub mod ai_info;
pub mod analysis;
#[cfg(test)]
mod array_operand_materialization_tests;
#[cfg(test)]
mod array_tests;
#[cfg(test)]
mod builtin_signature_tests;
mod builtins_visitor;
mod bytecode;
#[cfg(test)]
mod canonical_value_tests;
mod capture;
mod compiler;
pub mod conveyor;
pub mod conveyor_compile;
#[cfg(test)]
mod conveyor_tests;
pub mod db;
mod diagnostic;
pub mod editing;
pub mod equation_view;
pub mod errors;
#[cfg(test)]
mod keyword_ident_tests;
pub mod layout;
#[cfg(test)]
mod lookup_only_tests;
pub mod ltm;
pub mod ltm_agg;
pub mod ltm_augment;
pub mod ltm_dominance;
#[cfg(test)]
mod ltm_elementwise_tests;
pub mod ltm_finding;
#[cfg(any(test, feature = "test-support"))]
pub mod ltm_loop_scores_test_support;
pub mod ltm_post;
#[cfg(test)]
mod macro_expansion_tests;
#[cfg(test)]
mod mapped_reference_semantics_tests;
#[cfg(test)]
mod mismatched_read_tests;
mod model;
mod patch;
mod patch_dimensions;
mod patch_fields;
#[cfg(test)]
mod patch_sharing_tests;
#[cfg(test)]
mod per_element_gf_tests;
pub mod queue;
pub mod queue_compile;
#[cfg(test)]
mod results_tsv_tests;
#[cfg(test)]
mod rk_integration_tests;
#[cfg(test)]
mod round_builtin_tests;
pub mod save_check;
mod snapshot_arg;
#[cfg(test)]
mod subscript_out_of_range_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod test_common;
#[cfg(any(test, feature = "test-support"))]
pub mod testutils;
#[cfg(feature = "agent_tools")]
pub mod tools;
#[cfg(test)]
mod unfilled_equation_tests;
#[cfg(test)]
mod unit_checking_test;
mod units;
mod units_check;
mod units_infer;
mod variable;
mod vm;
// Bytecode-composition profiling for CompiledSimulation; a diagnostics-only
// sibling of `vm` kept separate purely for the per-file line cap.
mod vm_profile;
mod vm_vector_elm_map;
mod vm_vector_sort_order;
// WebAssembly code-generation backend: lowers the salsa-compiled
// `CompiledSimulation` bytecode (the same value `Vm::new` consumes) to a
// self-contained wasm module, as an alternative execution path to the bytecode
// VM. Validated in tests by executing the emitted module under a pure-Rust wasm
// interpreter and comparing against the VM.
pub mod wasmgen;

pub use self::patch::{
    PatchReport, Renamed, VariableRef, apply_patch, apply_patch_reporting, is_view_only_patch,
};
pub use self::queue_compile::build_sim;
#[cfg(feature = "test-support")]
pub use self::variable::parse_table;
pub use self::variable::{DepClassification, DepLag, DepOccurrence, SourceVariableKind, Variable};
pub use self::vm::{CompiledSimulation, Vm};
pub use self::vm_profile::BytecodeProfile;
