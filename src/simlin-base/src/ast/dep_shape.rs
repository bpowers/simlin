// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The shape of a name an equation references, which is all lowering needs to
//! know of it: [`LoweringScope`](super::LoweringScope) holds one for every name
//! of its equation, and the fragment compiler resolves the same equation's
//! references through the same map.

use std::sync::Arc;

use crate::common::{Canonical, Ident, IdentMap};
use crate::dimensions::Dimension;

/// What lowering needs to know about one name a fragment can reference: its
/// dimensions and whether it is a plain variable or a module instance.
#[derive(Debug, Clone, PartialEq)]
pub struct DepShape {
    /// The declared dimensions; empty for a scalar and for a module instance.
    pub dims: Vec<Dimension>,
    pub kind: DepKind,
}

/// The storage a dependency's name denotes.
#[derive(Debug, Clone, PartialEq)]
pub enum DepKind {
    /// An aux, flow, stock, lookup table or synthesized helper: one slot per
    /// element of `dims` (one slot when scalar).
    Var,
    /// A module instance: one block of the instantiated model's slots, laid
    /// out as `shape` says.
    Module { shape: Arc<ModelShape> },
}

impl DepShape {
    /// A plain variable over `dims` (scalar when empty).
    pub fn var(dims: Vec<Dimension>) -> Self {
        DepShape {
            dims,
            kind: DepKind::Var,
        }
    }

    /// A module instance of a model laid out as `shape`.
    pub fn module(shape: Arc<ModelShape>) -> Self {
        DepShape {
            dims: Vec::new(),
            kind: DepKind::Module { shape },
        }
    }

    /// Whether the name is a module instance. This is the one shape whose
    /// value depends on the LTM overlay -- it carries the sub-model's layout,
    /// which holds that model's LTM section under `On` -- so it is also what
    /// decides whether a fragment's memo is keyed on the overlay
    /// (`db::var_fragment::fragment_reads_module`).
    pub fn is_module(&self) -> bool {
        matches!(self.kind, DepKind::Module { .. })
    }

    /// The dimensions, or `None` for a scalar -- the form every arrayed-lowering
    /// decision in `compiler::context` branches on.
    pub fn dimensions(&self) -> Option<&[Dimension]> {
        if self.dims.is_empty() {
            None
        } else {
            Some(&self.dims)
        }
    }

    /// The slots this name occupies: the element count of a variable, the
    /// instantiated model's slot count for a module.
    pub fn size(&self) -> usize {
        match &self.kind {
            DepKind::Var => self.dims.iter().map(|d| d.len()).product::<usize>().max(1),
            DepKind::Module { shape } => shape.n_slots,
        }
    }
}

/// A sub-model's fixed layout together with each variable's shape: what a
/// cross-module reference resolves through (`db::model_shape` derives it from
/// `compute_layout` and the per-variable dimension queries).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelShape {
    pub vars: IdentMap<Ident<Canonical>, ShapeEntry>,
    pub n_slots: usize,
}

/// One sub-model variable: where its block starts inside the instance, and its
/// shape (a nested module instance carries the nested model's shape).
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeEntry {
    pub offset: usize,
    pub shape: DepShape,
}
