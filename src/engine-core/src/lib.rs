// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

use core;
#[macro_use]
use lazy_static;
use prost;
use quick_xml;
use regex;
use serde;
use unicode_xid;
#[macro_use]
use float_cmp;

#[macro_use]
mod common;
mod ast;
mod datamodel;
mod project;
mod project_io {
    include!(concat!(env!("OUT_DIR"), "/project_io.rs"));
}
mod equation {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/equation.rs"));
}
mod builtins;
mod builtins_visitor;
mod model;
mod sim;
mod token;
mod variable;
mod xmile;

mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}

mod interpreter;

pub use self::common::Result;
pub use self::xmile::from_reader;
