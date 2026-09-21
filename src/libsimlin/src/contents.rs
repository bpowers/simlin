// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A project's datamodel, with its revision and the indexes the editing entry
//! points derive from it.
//!
//! An index is right only while the datamodel it was built from stands, so the
//! datamodel is reached only through [`ProjectContents`]: it derefs to the
//! datamodel, and the two ways to change it -- a mutable borrow (`DerefMut`)
//! and a replacement (`ProjectContents::replace`) -- each drop every index and
//! advance the revision. No mutating entry point has to remember to invalidate
//! an index or count a change, because there is no other way to change the
//! contents. The indexes
//! and the revision live under the datamodel's own lock
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
//!
//! The revision is what a host compares to learn whether a project changed:
//! equal revisions mean equal contents. The converse does not hold -- a
//! mutable borrow that ends up changing nothing (a view-only patch that fails
//! part way, say) still advances it -- which errs the safe way for every
//! reader, since a spurious advance costs a re-read and a missed one would
//! serve stale state.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use simlin_engine::{datamodel, editing};

/// A project's datamodel, its revision, and the indexes derived from it.
pub struct ProjectContents {
    /// Shared with every project whose contents were replaced from this one,
    /// until one of them is edited.
    datamodel: Arc<datamodel::Project>,
    /// How many times the contents have changed (a mutable borrow of the
    /// datamodel, or a replacement) since the project was opened.
    revision: u64,
    /// Hit indexes by model name, each built from `datamodel` when first asked
    /// for, and all dropped by any mutable borrow of `datamodel`.
    hit_indexes: HashMap<String, editing::HitIndex>,
}

impl ProjectContents {
    pub(crate) fn new(datamodel: datamodel::Project) -> ProjectContents {
        ProjectContents {
            datamodel: Arc::new(datamodel),
            revision: 0,
            hit_indexes: HashMap::new(),
        }
    }

    /// The project's revision: advanced by every mutable borrow of the
    /// datamodel and by every replacement, so two reads that see the same
    /// revision see the same contents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The datamodel itself, for another project to share or for a caller
    /// that needs it after releasing the lock.
    pub(crate) fn shared(&self) -> Arc<datamodel::Project> {
        Arc::clone(&self.datamodel)
    }

    /// Makes `datamodel` these contents, dropping every index and advancing
    /// the revision: a replacement that shares the datamodel it is given
    /// rather than copying it.
    pub(crate) fn replace(&mut self, datamodel: Arc<datamodel::Project>) {
        self.hit_indexes.clear();
        self.revision += 1;
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
    /// Drops every index and advances the revision before the datamodel is
    /// lent out, and copies the datamodel first when another project shares
    /// it: whatever the borrower changes, no index built from the datamodel as
    /// it was survives, no reader keeps the revision it read the old datamodel
    /// at, and no other project sees the change.
    fn deref_mut(&mut self) -> &mut datamodel::Project {
        self.hit_indexes.clear();
        self.revision += 1;
        Arc::make_mut(&mut self.datamodel)
    }
}

#[cfg(test)]
#[path = "contents_tests.rs"]
mod tests;
