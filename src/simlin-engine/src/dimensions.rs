// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::Ident;
use crate::datamodel;

#[derive(Clone, PartialEq, Debug)]
pub struct Dimension {}

#[derive(Clone, PartialEq, Debug)]
pub struct DimensionsContext {
    dimensions: HashMap<Ident, Dimension>,
}
