// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::rc::Rc;

use super::model;

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub name: String,
    pub models: HashMap<String, Rc<model::Model>>,
}
