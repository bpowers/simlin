// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::tools::test_support::inventory;

fn model() -> crate::datamodel::Model {
    inventory().build_datamodel().models.remove(0)
}

#[test]
fn string_similarity_is_one_for_equal_strings_and_falls_with_each_edit() {
    assert_eq!(string_similarity("inventory", "inventory"), 1.0);
    assert_eq!(string_similarity("", ""), 1.0);
    assert_eq!(string_similarity("abc", ""), 0.0);
    assert_eq!(string_similarity("abc", "xyz"), 0.0);
    let one_edit = string_similarity("inventory", "invntory");
    let two_edits = string_similarity("inventory", "invntry");
    assert!(
        one_edit > two_edits && two_edits > 0.5,
        "{one_edit} {two_edits}"
    );
    // Characters, not bytes: one accented letter is one edit.
    assert_eq!(string_similarity("café", "cafe"), 0.75);
}

#[test]
fn words_ignore_case_spaces_and_underscores_and_only_a_query_drops_stop_words() {
    assert_eq!(words("Room Temperature"), ["room", "temperature"]);
    assert_eq!(words("room_temperature"), ["room", "temperature"]);
    assert_eq!(words("stock a"), ["stock", "a"], "a name keeps every word");
    assert_eq!(
        query_words("the level of inventory"),
        ["level", "inventory"]
    );
    // A phrase that is nothing but stop words keeps them.
    assert_eq!(query_words("the"), ["the"]);
    assert!(words("  ").is_empty());
}

#[test]
fn a_name_resolves_whatever_its_case_spacing_or_underscores() {
    let model = model();
    for (query, expected) in [
        ("Inventory", "Inventory"),
        ("INVENTORY", "Inventory"),
        (" inventory ", "Inventory"),
        ("desired inventory", "desired_inventory"),
        ("Desired_Inventory", "desired_inventory"),
    ] {
        let var = resolve(&model, query).unwrap_or_else(|s| panic!("{query}: {s:?}"));
        assert_eq!(var.get_ident(), expected, "{query}");
    }
}

/// A name that does not resolve is answered with the closest names, one row per
/// way a name goes wrong.
#[test]
fn a_name_that_does_not_resolve_suggests_the_variable_it_most_likely_meant() {
    let model = model();
    for (query, expected, how) in [
        ("invntory", "Inventory", "a letter dropped"),
        ("inventroy", "Inventory", "letters swapped"),
        ("desiredinventory", "desired_inventory", "a space missing"),
        ("inventory desired", "desired_inventory", "words reordered"),
        ("shipment", "shipments", "singular for plural"),
        ("adjustment", "adjustment_time", "part of the name"),
        (
            "desired inventery",
            "desired_inventory",
            "a misspelled word",
        ),
    ] {
        let suggestions = resolve(&model, query).expect_err(query);
        assert_eq!(
            suggestions.first().map(String::as_str),
            Some(expected),
            "{how}: {query} -> {suggestions:?}"
        );
        assert!(suggestions.len() <= MAX_SUGGESTIONS, "{query}");
    }
}

#[test]
fn a_name_like_nothing_in_the_model_suggests_nothing() {
    let model = model();
    let suggestions = resolve(&model, "zzzz qqq").expect_err("nothing is named that");
    assert!(suggestions.is_empty(), "{suggestions:?}");
}

#[test]
fn a_description_finds_a_variable_through_its_documentation() {
    let model = model();
    let ranked = rank(&model, "widgets on hand");
    assert_eq!(ranked[0].1.get_ident(), "Inventory");
    assert!(
        ranked[0].0 >= 0.7,
        "documentation that covers the phrase scores well: {}",
        ranked[0].0
    );
}

#[test]
fn ranking_scores_every_variable_closest_first_with_ties_broken_by_name() {
    let project = crate::test_common::TestProject::new("ties")
        .aux("rate_b", "1", None)
        .aux("rate_a", "1", None)
        .aux("stock level", "1", None)
        .build_datamodel();
    let model = &project.models[0];
    let ranked = rank(model, "rate");
    let names: Vec<&str> = ranked.iter().map(|(_, v)| v.get_ident()).collect();
    assert_eq!(names, ["rate_a", "rate_b", "stock level"]);
    assert_eq!(ranked[0].0, ranked[1].0, "the two rates tie");
    assert!(ranked[1].0 > ranked[2].0);
}
