// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Test-only salsa execution counting: which tracked queries actually ran a
//! body, and how many times, over a measured region.
//!
//! `db::fragment_compile`'s `note_fragment_execution` answers the same question
//! for the four fragment compilers by instrumenting their bodies. This module
//! answers it for EVERY tracked query at once, without touching any body:
//! salsa emits an `EventKind::WillExecute` immediately before it runs one, and
//! a [`salsa::Storage`] built with an event callback receives them.
//!
//! Why not memo pointers or values: salsa backdates a re-executed query whose
//! new value compares equal, keeping the memo address, so "the value is the
//! same" and "the memo is the same object" are both true whether or not the
//! expensive body re-ran. Only the event says which.
//!
//! Scope: an execution count is a property of a whole database, so a probe
//! measures one `SimlinDb` from [`ProbedDb::new`] to the end of its life. Salsa
//! runs a query on whatever thread demanded it, so the log is behind a mutex
//! rather than thread-local -- unlike `note_fragment_execution`, which sits
//! inside bodies and is charged per thread.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use salsa::{Database, DatabaseKeyIndex, Event, EventKind};

use super::SimlinDb;

/// A `SimlinDb` that records every tracked-query body entry.
///
/// The db is reached through [`ProbedDb::db`] / [`ProbedDb::db_mut`] and used
/// exactly like any other; [`ProbedDb::reset`] starts a measured region and
/// [`ProbedDb::counts`] reads it back as query name -> execution count.
pub(crate) struct ProbedDb {
    db: SimlinDb,
    log: Arc<Mutex<Vec<DatabaseKeyIndex>>>,
}

impl ProbedDb {
    pub(crate) fn new() -> Self {
        let log: Arc<Mutex<Vec<DatabaseKeyIndex>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        let storage = salsa::Storage::new(Some(Box::new(move |event: Event| {
            if let EventKind::WillExecute { database_key } = event.kind {
                // The whole key, not just its ingredient: "13 executions" and
                // "13 DISTINCT keys executed once each" are different findings,
                // and only the second says a re-keying is what re-ran the
                // query. Resolving the ingredient to a NAME needs the database,
                // which this callback does not hold, so that is deferred to
                // `counts`.
                sink.lock()
                    .expect("execution-probe log poisoned")
                    .push(database_key);
            }
        })));
        ProbedDb {
            db: SimlinDb::with_storage(storage),
            log,
        }
    }

    pub(crate) fn db(&self) -> &SimlinDb {
        &self.db
    }

    pub(crate) fn db_mut(&mut self) -> &mut SimlinDb {
        &mut self.db
    }

    /// Start (or restart) a measured region, discarding what came before.
    /// Call it after the fixture is built and primed, so setup is not charged
    /// to the region.
    pub(crate) fn reset(&self) {
        self.log
            .lock()
            .expect("execution-probe log poisoned")
            .clear();
    }

    /// Query name -> `(bodies run, distinct keys among them)` since the last
    /// [`ProbedDb::reset`].
    ///
    /// The name is salsa's ingredient debug name: the tracked function's own
    /// name. Both halves are reported because they answer different questions
    /// -- one key re-running `n` times is a query being re-demanded, `n` keys
    /// running once each is a query having been re-keyed or newly demanded.
    pub(crate) fn counts(&self) -> BTreeMap<String, (usize, usize)> {
        let log = self.log.lock().expect("execution-probe log poisoned");
        let mut runs: BTreeMap<String, usize> = BTreeMap::new();
        let mut keys: BTreeMap<String, HashSet<DatabaseKeyIndex>> = BTreeMap::new();
        for key in log.iter() {
            let name = self
                .db
                .ingredient_debug_name(key.ingredient_index())
                .into_owned();
            *runs.entry(name.clone()).or_default() += 1;
            keys.entry(name).or_default().insert(*key);
        }
        runs.into_iter()
            .map(|(name, n)| {
                let distinct = keys[&name].len();
                (name, (n, distinct))
            })
            .collect()
    }
}

impl Default for ProbedDb {
    fn default() -> Self {
        ProbedDb::new()
    }
}
