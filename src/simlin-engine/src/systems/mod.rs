// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Systems format parser.
//!
//! Parses the line-oriented systems format (`.txt` files) into a
//! `SystemsModel` intermediate representation with stock declarations,
//! flow definitions (Rate, Conversion, Leak), and formula expressions.

pub mod ast;
mod lexer;
mod parser;
pub mod translate;

pub use parser::parse;
