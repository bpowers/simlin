// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A project's datamodel, with the indexes the editing entry points derive from
//! it.
//!
//! An index is right only while the datamodel it was built from stands, so the
//! datamodel is reached only through [`ProjectContents`]: it derefs to the
//! datamodel, and a mutable borrow of the datamodel (`DerefMut`) advances the
//! project's revision before lending the datamodel out. No mutating entry point
//! has to remember to invalidate an index, because there is no other way to
//! mutate the datamodel.
//!
//! The indexes are published ([`Published`]) under a lock of their own, each
//! with the revision it was built at, so a hit test reads one without waiting
//! for the datamodel's lock, which an edit holds through its compile. An index
//! is built and published only under the datamodel's lock, where the revision
//! cannot move, so a published index describes the datamodel exactly while its
//! revision is still the project's. A reader compares the two; a stale or
//! missing index sends it to the datamodel's lock to build one. The lock order
//! is the datamodel, then the published indexes, and the published indexes are
//! never held with the db.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use simlin_engine::{datamodel, editing};

/// The indexes derived from a project's datamodel, published for readers that
/// do not hold the datamodel's lock.
#[derive(Default)]
pub(crate) struct Published {
    /// The datamodel's revision, advanced by every mutable borrow of it before
    /// the borrower can change anything.
    revision: AtomicU64,
    /// Hit indexes by model name, each with the revision it was built at.
    hit_indexes: Mutex<HashMap<String, (u64, Arc<editing::HitIndex>)>>,
}

impl Published {
    /// The published hit index of `model_name` while it is current: built at the
    /// revision the datamodel still has. Locks only the published indexes.
    pub(crate) fn hit_index(&self, model_name: &str) -> Option<Arc<editing::HitIndex>> {
        let (built, index) = self.hit_indexes.lock().unwrap().get(model_name).cloned()?;
        (built == self.revision.load(Ordering::SeqCst)).then_some(index)
    }
}

/// A project's datamodel and the indexes derived from it.
pub struct ProjectContents {
    datamodel: datamodel::Project,
    /// Shared with `SimlinProject::published`, where readers find it without
    /// this lock.
    published: Arc<Published>,
}

impl ProjectContents {
    pub(crate) fn new(datamodel: datamodel::Project, published: Arc<Published>) -> ProjectContents {
        ProjectContents {
            datamodel,
            published,
        }
    }

    /// The hit index of `model_name`'s first stock-and-flow view: the current
    /// published one, or one built from the datamodel and published before this
    /// returns. The caller holds the datamodel's lock, so the revision the index
    /// is published at is the revision it was built from. Fails where the view
    /// cannot be resolved (a missing model, a model with no view), and publishes
    /// nothing then.
    pub(crate) fn publish_hit_index(
        &self,
        model_name: &str,
    ) -> Result<Arc<editing::HitIndex>, String> {
        if let Some(index) = self.published.hit_index(model_name) {
            return Ok(index);
        }
        let revision = self.published.revision.load(Ordering::SeqCst);
        let index = Arc::new(editing::HitIndex::new(&self.datamodel, model_name)?);
        self.published
            .hit_indexes
            .lock()
            .unwrap()
            .insert(model_name.to_string(), (revision, Arc::clone(&index)));
        Ok(index)
    }
}

impl Deref for ProjectContents {
    type Target = datamodel::Project;

    fn deref(&self) -> &datamodel::Project {
        &self.datamodel
    }
}

impl DerefMut for ProjectContents {
    /// Advances the revision before the datamodel is lent out: whatever the
    /// borrower changes, no index built from the datamodel as it was is current
    /// any longer.
    fn deref_mut(&mut self) -> &mut datamodel::Project {
        self.published.revision.fetch_add(1, Ordering::SeqCst);
        &mut self.datamodel
    }
}

#[cfg(test)]
#[path = "contents_tests.rs"]
mod tests;
