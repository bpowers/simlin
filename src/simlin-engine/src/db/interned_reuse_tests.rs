// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An edit history that reuses interned slots never reaches salsa's
//! validation assertion.
//!
//! Salsa collects an interned value no revision has used for a while and
//! reuses its slot for another. Reading a collectable value's fields, or a memo
//! keyed on it, without the value having been interned or validated in the
//! current revision is a panic (`Value::assert_validated` in salsa's
//! `interned.rs`), and a panic is a process abort in a release build.
//!
//! This database's keys are collectable: `LtmLinkId` and `ModuleInputSet` keep
//! salsa's default `revisions`, and so do the argument tuples salsa interns for
//! every query that takes more than one argument, which is most of them. What
//! keeps the assertion out of reach is that no memoized value holds an interned
//! id, so an id is only ever a query's key, a dependency edge, or a local under
//! the borrow that interned it.
//!
//! A database that lives for one compile never reuses a slot (collection starts
//! after the third revision that interns anything, and takes a slot nothing has
//! used for three), so the ordinary suite passing says nothing about this.
//! These tests drive the database the way a host does over many edits and count
//! salsa's own `DidReuseInternedValue` events: a history in which no slot was
//! reused fails, because it would not have exercised the property.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use salsa::{Database, Event, EventKind, IngredientIndex};

use super::{LtmOverlay, SimlinDb, collect_all_diagnostics, compile_project_incremental};
use crate::datamodel;
use crate::test_common::TestProject;

/// A database that records which ingredient each reused interned slot belongs
/// to.
fn db_recording_reuse() -> (SimlinDb, Arc<Mutex<Vec<IngredientIndex>>>) {
    let reused: Arc<Mutex<Vec<IngredientIndex>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&reused);
    let storage = salsa::Storage::new(Some(Box::new(move |event: Event| {
        if let EventKind::DidReuseInternedValue { key, .. } = event.kind {
            sink.lock()
                .expect("reuse log poisoned")
                .push(key.ingredient_index());
        }
    })));
    (SimlinDb::with_storage(storage), reused)
}

/// A model whose interned keys change with `round`:
///
/// * the rate's NAME rotates, so the name-keyed projections
///   (`model_variable_by_name` and friends) and the `LtmLinkId`s of the links
///   through it are keys one round interns and a later one abandons;
/// * the smooth's call shape alternates between two and three arguments, and
///   a stdlib delay appears and disappears;
/// * an explicit module instance is wired to a pair of its sub-model's
///   `PORTS` ports that changes every round, so its bound-port set
///   (`ModuleInputSet`, part of the key of the dependency graph, every
///   fragment and the assembled module) is one of 55, and rounds keep
///   interning sets no earlier round did while the sets of rounds long past
///   sit unused.
fn churn_project(round: usize) -> datamodel::Project {
    const PORTS: usize = 11;
    let k = round % PORTS;
    let rate = format!("rate_{k}");
    let perceived = if round.is_multiple_of(2) {
        "SMTH1(pop, 3)".to_string()
    } else {
        "SMTH1(pop, 3, 50)".to_string()
    };
    let mut builder = TestProject::new("churn")
        .with_sim_time(0.0, 4.0, 0.5)
        .stock("pop", "100", &["births"], &["deaths"], None)
        .flow("births", &format!("pop * {rate}"), None)
        .flow("deaths", "pop * crowding / 20", None)
        .aux(&rate, &format!("0.0{} * (2 - crowding)", k + 1), None)
        .aux("crowding", "(perceived + inst.out) / 2000", None)
        .aux("perceived", &perceived, None);
    if round.is_multiple_of(3) {
        builder = builder
            .aux(&format!("lagged_{k}"), "DELAY3(births, 2)", None)
            .aux("readout", &format!("lagged_{k} + pop"), None);
    }
    let mut project = builder.build_datamodel();

    let port = |n: usize| format!("p_{n}");
    let wired = [k, (k + 1 + (round / PORTS) % (PORTS - 1)) % PORTS];
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "inst".to_string(),
            model_name: "sub".to_string(),
            documentation: String::new(),
            units: None,
            references: wired
                .iter()
                .map(|n| datamodel::ModuleReference {
                    src: "pop".to_string(),
                    dst: format!("inst.{}", port(*n)),
                })
                .collect(),
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        }));

    let sub_aux = |ident: String, equation: String, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident,
            equation: datamodel::Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    let mut sub_vars: Vec<datamodel::Variable> = (0..PORTS)
        .map(|n| sub_aux(port(n), "1".to_string(), true))
        .collect();
    let sum = (0..PORTS).map(port).collect::<Vec<_>>().join(" + ");
    sub_vars.push(sub_aux("out".to_string(), sum, false));
    project.models.push(datamodel::Model {
        name: "sub".to_string(),
        sim_specs: None,
        variables: sub_vars.into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project
}

/// What a host asks of the db after an edit, with and without the LTM
/// overlay: the compiled simulation and every diagnostic.
fn query_as_a_host_does(db: &SimlinDb, project: super::SourceProject) {
    for overlay in [LtmOverlay::Off, LtmOverlay::On] {
        let compiled = compile_project_incremental(db, project, "main", overlay);
        assert!(
            compiled.is_ok(),
            "the churn model compiles: {:?}",
            compiled.err()
        );
        let _ = collect_all_diagnostics(db, project, overlay);
    }
}

/// One edit, as a host makes it: the sync, the queries, then the memo release
/// libsimlin's `DbLock` runs when it drops.
fn edit(db: &mut SimlinDb, round: usize) {
    let project = db.sync(&churn_project(round));
    query_as_a_host_does(db, project);
    db.release_replaced_memos();
}

/// How many slots each ingredient reused, by the ingredient's name.
fn reuse_by_ingredient(db: &SimlinDb, reused: &[IngredientIndex]) -> BTreeMap<String, usize> {
    let mut by_ingredient: BTreeMap<String, usize> = BTreeMap::new();
    for index in reused {
        *by_ingredient
            .entry(db.ingredient_debug_name(*index).into_owned())
            .or_default() += 1;
    }
    by_ingredient
}

/// A short history, small enough for the default suite: enough edits for
/// salsa to start collecting, and for slots of the per-variable argument
/// tuples to be reused.
#[test]
fn a_few_edits_reuse_interned_slots_without_reaching_the_assertion() {
    let (mut db, reused) = db_recording_reuse();
    for round in 0..12 {
        edit(&mut db, round);
    }
    let reused = reused.lock().expect("reuse log poisoned");
    assert!(
        !reused.is_empty(),
        "no interned slot was reused, so this history says nothing about the assertion"
    );
}

/// A long history, with both kinds of key this database interns reused: an
/// interned struct and the argument tuples of the central queries.
///
/// Which slots a history reuses is not a property of the history alone. Salsa
/// keeps each interned table in `4 * available_parallelism` shards (rounded up
/// to a power of two), reuses a slot only for a key that hashes into the same
/// shard, and looks only at the shard's least-recently-interned entry
/// (`find_reusable_slot`); an entry a memo's verification revalidates is
/// refreshed where it sits (`maybe_changed_after`), so it can stand at that
/// end of a shard and hold the slots behind it for as long as it is
/// revalidated. The fewer the shards, the likelier that is. The ingredients
/// required below are the ones this history reuses at every shard count from
/// 4 to 128 (1 to 32 cores). `LtmLinkId` is not among them: this history
/// reuses its slots dozens of times at 64 shards and up, and once or never
/// below that, so on a small machine it exercises `LtmLinkId` as a live key
/// and not as a reused one.
///
/// Its middle phase makes no input write at all. `release_replaced_memos`
/// opens a synthetic revision on every 256th consecutive call (salsa counts
/// exclusive accesses in a `u8`), so the keys the queries there read are
/// validated in revisions no edit opened, and the edits after it reuse the
/// slots that went stale across them.
#[test]
#[ignore = "240 edits and 1,536 quiet releases of a small model, queried under both overlays; run under the gates profile"]
fn an_edit_history_that_reuses_interned_slots_never_reaches_the_assertion() {
    let (mut db, reused) = db_recording_reuse();

    for round in 0..120 {
        edit(&mut db, round);
    }

    let project = db.sync(&churn_project(0));
    for _ in 0..(256 * 6) {
        db.release_replaced_memos();
        query_as_a_host_does(&db, project);
    }

    for round in 120..240 {
        edit(&mut db, round);
    }

    let reused = reused.lock().expect("reuse log poisoned");
    let by_ingredient = reuse_by_ingredient(&db, &reused);
    // The keys whose reuse matters most: an explicit interned struct, and the
    // argument tuples of the queries every compile goes through. A history
    // that stops reusing one of them has stopped testing it.
    for ingredient in [
        "ModuleInputSet",
        "compile_var_fragment::interned_arguments",
        "assemble_module::interned_arguments",
        "variable_direct_dependencies::interned_arguments",
    ] {
        assert!(
            by_ingredient.get(ingredient).copied().unwrap_or(0) > 0,
            "no `{ingredient}` slot was reused; reused: {by_ingredient:#?}"
        );
    }
}
