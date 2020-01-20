// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

extern crate lalrpop;

fn main() {
    prost_build::compile_protos(&["src/ast.proto"], &["src/"]).unwrap();
    // lalrpop::process_root().unwrap();
}
