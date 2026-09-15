// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A project's datamodel, with the indexes the editing entry points derive from
//! it.
//!
//! An index is right only while the datamodel it was built from stands, so the
//! datamodel is reached only through [`ProjectContents`]: it derefs to the
//! datamodel, and a mutable borrow of the datamodel (`DerefMut`) drops every
//! index before lending the datamodel out. No mutating entry point has to
//! remember to invalidate one, because there is no other way to mutate the
//! datamodel. The indexes live under the datamodel's own lock
//! (`SimlinProject::datamodel`), so an entry point reads an index built from
//! exactly the datamodel it locked, and they add no lock of their own to the
//! project-wide datamodel-then-db order.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use simlin_engine::{datamodel, editing};

/// A project's datamodel and the indexes derived from it.
pub struct ProjectContents {
    datamodel: datamodel::Project,
    /// Hit indexes by model name, each built from `datamodel` when first asked
    /// for, and all dropped by any mutable borrow of `datamodel`.
    hit_indexes: HashMap<String, editing::HitIndex>,
}

impl ProjectContents {
    pub(crate) fn new(datamodel: datamodel::Project) -> ProjectContents {
        ProjectContents {
            datamodel,
            hit_indexes: HashMap::new(),
        }
    }

    /// The hit index of `model_name`'s first stock-and-flow view: the one
    /// built since the datamodel last changed, or a new one. Fails where the
    /// view cannot be resolved (a missing model, a model with no view), and
    /// caches nothing then.
    pub(crate) fn hit_index(&mut self, model_name: &str) -> Result<&editing::HitIndex, String> {
        if !self.hit_indexes.contains_key(model_name) {
            let index = editing::HitIndex::new(&self.datamodel, model_name)?;
            self.hit_indexes.insert(model_name.to_string(), index);
        }
        Ok(&self.hit_indexes[model_name])
    }

    /// Whether a hit index of `model_name` is cached.
    #[cfg(test)]
    pub(crate) fn has_hit_index(&self, model_name: &str) -> bool {
        self.hit_indexes.contains_key(model_name)
    }
}

impl Deref for ProjectContents {
    type Target = datamodel::Project;

    fn deref(&self) -> &datamodel::Project {
        &self.datamodel
    }
}

impl DerefMut for ProjectContents {
    /// Drops every index before the datamodel is lent out: whatever the
    /// borrower changes, no index built from the datamodel as it was survives.
    fn deref_mut(&mut self) -> &mut datamodel::Project {
        self.hit_indexes.clear();
        &mut self.datamodel
    }
}

#[cfg(test)]
#[path = "contents_tests.rs"]
mod tests;
