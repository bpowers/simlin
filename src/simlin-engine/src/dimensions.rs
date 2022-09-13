// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::Ident;
use crate::datamodel;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NamedDimension {
    elements: Vec<String>,
    indexed_elements: HashMap<Ident, usize>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
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
                        // system dynamic indexes are 1-indexed
                        .map(|(i, elem)| (elem.clone(), i + 1))
                        .collect(),
                    elements,
                },
            ),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DimensionsContext {
    dimensions: HashMap<Ident, Dimension>,
}

impl DimensionsContext {
    pub(crate) fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        DimensionsContext {
            dimensions: dimensions
                .iter()
                .map(|dim| (dim.name().to_owned(), Dimension::from(dim.clone())))
                .collect(),
        }
    }

    pub(crate) fn lookup(&self, element: &str) -> Option<u32> {
        if let Some(pos) = element.find('·') {
            let dimension_name = &element[..pos];
            let element_name = &element[pos + '·'.len_utf8()..];
            if let Some(Dimension::Named(_, dimension)) = self.dimensions.get(dimension_name) {
                if let Some(off) = dimension.indexed_elements.get(element_name) {
                    return Some(*off as u32);
                }
            }
        }
        None
    }
}
