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
//!
//! The datamodel is shared: `simlin_project_replace_contents` gives the
//! destination the source's datamodel itself, not a copy, so a host's copy of a
//! project (an undo step's snapshot, a save's) costs a reference count until
//! one of the two is edited. The same mutable borrow that drops the indexes
//! copies a shared datamodel first (`Arc::make_mut`), so an edit of one project
//! never reaches another, and a datamodel nothing else holds is edited in place.
//! That copy still shares every variable and view element with the original
//! (`datamodel::SharedVec`), so an edit costs a pointer per element plus what
//! it changes.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use simlin_engine::{datamodel, editing};

/// A project's datamodel and the indexes derived from it.
pub struct ProjectContents {
    /// Shared with every project whose contents were replaced from this one,
    /// until one of them is edited.
    datamodel: Arc<datamodel::Project>,
    /// Hit indexes by model name, each built from `datamodel` when first asked
    /// for, and all dropped by any mutable borrow of `datamodel`.
    hit_indexes: HashMap<String, editing::HitIndex>,
}

impl ProjectContents {
    pub(crate) fn new(datamodel: datamodel::Project) -> ProjectContents {
        ProjectContents {
            datamodel: Arc::new(datamodel),
            hit_indexes: HashMap::new(),
        }
    }

    /// The datamodel itself, for another project to share or for a caller
    /// that needs it after releasing the lock.
    pub(crate) fn shared(&self) -> Arc<datamodel::Project> {
        Arc::clone(&self.datamodel)
    }

    /// Makes `datamodel` these contents, dropping every index: a replacement
    /// that shares the datamodel it is given rather than copying it.
    pub(crate) fn replace(&mut self, datamodel: Arc<datamodel::Project>) {
        self.hit_indexes.clear();
        self.datamodel = datamodel;
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
    /// Drops every index before the datamodel is lent out, and copies the
    /// datamodel first when another project shares it: whatever the borrower
    /// changes, no index built from the datamodel as it was survives, and no
    /// other project sees the change.
    fn deref_mut(&mut self) -> &mut datamodel::Project {
        self.hit_indexes.clear();
        Arc::make_mut(&mut self.datamodel)
    }
}

#[cfg(test)]
#[path = "contents_tests.rs"]
mod tests;
