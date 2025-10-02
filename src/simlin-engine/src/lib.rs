// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

pub use prost;

mod ast;
pub mod common;
pub mod datamodel;
#[allow(clippy::derive_partial_eq_without_eq)]
pub mod project_io {
    include!(concat!(env!("OUT_DIR"), "/project_io.rs"));
}
pub mod json;
pub mod serde;
mod equation {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/equation.rs"));
}
pub mod builtins;
mod builtins_visitor;
mod compiler;
mod dimensions;
mod model;
mod patch;
mod token;
mod variable;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}
pub mod ai_info;
#[cfg(test)]
mod array_tests;
mod bytecode;
pub mod interpreter;
pub mod ltm;
pub mod ltm_augment;
mod project;
pub mod test_common;
#[cfg(test)]
mod testutils;
#[cfg(test)]
mod unit_checking_test;
mod units;
mod units_check;
mod units_infer;
mod vm;

pub use self::common::{Error, ErrorCode, Result, canonicalize};
pub use self::interpreter::Simulation;
pub use self::model::{ModelStage1, resolve_non_private_dependencies};
pub use self::patch::apply_patch;
pub use self::project::Project;
pub use self::variable::{Variable, identifier_set};
pub use self::vm::Method;
pub use self::vm::Results;
pub use self::vm::Specs as SimSpecs;
pub use self::vm::Vm;
