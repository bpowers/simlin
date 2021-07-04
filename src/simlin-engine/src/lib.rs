// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

pub use prost;

mod ast;
pub mod common;
pub mod datamodel;
pub mod project_io {
    include!(concat!(env!("OUT_DIR"), "/project_io.rs"));
}
pub mod serde;
mod equation {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/equation.rs"));
}
mod builtins;
mod builtins_visitor;
mod compiler;
mod model;
mod token;
mod variable;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}
mod bytecode;
mod interpreter;
mod project;
mod units;
mod vm;

pub use self::common::{canonicalize, quoteize, Error, ErrorCode, Ident, Result};
pub use self::compiler::Simulation;
pub use self::project::Project;
pub use self::variable::Variable;
pub use self::vm::Method;
pub use self::vm::Results;
pub use self::vm::Specs as SimSpecs;
pub use self::vm::Vm;
