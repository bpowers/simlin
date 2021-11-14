// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::Ident;
use crate::datamodel;

#[derive(Clone, PartialEq, Debug)]
pub struct NamedDimension {
    elements: Vec<String>,
    indexed_elements: HashMap<Ident, usize>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Dimension {
    Indexed(Ident, u32),
    Named(Ident, NamedDimension),
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dim: datamodel::Dimension) -> Dimension {
        match dim {
            datamodel::Dimension::Indexed(name, size) => Dimension::Indexed(name, size),
            datamodel::Dimension::Named(name, elements) => Dimension::Named(
                name,
                NamedDimension {
                    indexed_elements: elements
                        .iter()
                        .enumerate()
                        .map(|(i, elem)| (elem.clone(), i))
                        .collect(),
                    elements,
                },
            ),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct DimensionsContext {
    dimensions: HashMap<Ident, Dimension>,
}

impl DimensionsContext {
    pub fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        DimensionsContext {
            dimensions: dimensions
                .iter()
                .map(|dim| (dim.name().to_owned(), Dimension::from(dim.clone())))
                .collect(),
        }
    }
}
