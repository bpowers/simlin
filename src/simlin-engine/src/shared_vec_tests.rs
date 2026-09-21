// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;

fn numbers() -> SharedVec<String> {
    ["zero", "one", "two", "three", "four"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// How many positions of `a` and `b` hold the same allocation.
fn shared(a: &SharedVec<String>, b: &SharedVec<String>) -> usize {
    a.addresses()
        .iter()
        .zip(b.addresses())
        .filter(|(x, y)| **x == *y)
        .count()
}

#[test]
fn a_clone_shares_every_element() {
    let original = numbers();
    let copy = original.clone();
    assert_eq!(shared(&original, &copy), 5);
    assert!(copy == original);
}

#[test]
fn get_mut_copies_only_the_element_it_returns() {
    let original = numbers();
    let mut copy = original.clone();
    copy.get_mut(3).unwrap().push('!');
    assert_eq!(copy[3], "three!");
    assert_eq!(original[3], "three", "the original keeps its element");
    assert_eq!(shared(&original, &copy), 4);
}

#[test]
fn find_mut_copies_only_the_element_it_finds() {
    let original = numbers();
    let mut copy = original.clone();
    copy.find_mut(|s| s.starts_with('t')).unwrap().push('!');
    assert_eq!(copy.to_vec(), ["zero", "one", "two!", "three", "four"]);
    assert_eq!(shared(&original, &copy), 4);
    assert!(copy.find_mut(|s| s == "five").is_none());
}

#[test]
fn replace_puts_a_value_in_place_and_leaves_the_rest_shared() {
    let original = numbers();
    let mut copy = original.clone();
    copy.replace(0, "nil".to_string());
    assert_eq!(copy[0], "nil");
    assert_eq!(original[0], "zero");
    assert_eq!(shared(&original, &copy), 4);
}

#[test]
fn edit_where_copies_only_the_elements_it_picks() {
    let original = numbers();
    let mut copy = original.clone();
    copy.edit_where(|s| s.len() == 4, |s| s.make_ascii_uppercase());
    assert_eq!(copy.to_vec(), ["ZERO", "one", "two", "three", "FOUR"]);
    assert_eq!(shared(&original, &copy), 3);
}

#[test]
fn edit_each_keeps_what_it_leaves_unchanged_shared() {
    let original = numbers();
    let mut copy = original.clone();
    // The edit visits every element but changes only those holding an `o`
    // after their first letter; the others come out equal and stay shared.
    copy.edit_each(|s| {
        if s[1..].contains('o') {
            *s = s.replace('o', "0");
        }
    });
    assert_eq!(copy.to_vec(), ["zer0", "one", "tw0", "three", "f0ur"]);
    assert_eq!(shared(&original, &copy), 2);
}

#[test]
fn edit_each_edits_an_element_nothing_else_holds_in_place() {
    let mut alone = numbers();
    let before = alone.addresses();
    alone.edit_each(|s| s.push('!'));
    assert_eq!(alone[4], "four!");
    assert_eq!(alone.addresses(), before, "no element was copied");
}

#[test]
fn rewrite_hands_over_a_plain_vector_and_the_result_shares_nothing() {
    let original = numbers();
    let mut copy = original.clone();
    let len = copy.rewrite(|plain| {
        plain.reverse();
        plain.len()
    });
    assert_eq!(len, 5);
    assert_eq!(copy.to_vec(), ["four", "three", "two", "one", "zero"]);
    assert_eq!(original.to_vec(), ["zero", "one", "two", "three", "four"]);
    let original_addresses = original.addresses();
    assert!(
        copy.addresses()
            .iter()
            .all(|a| !original_addresses.contains(a))
    );
}

#[test]
fn retain_and_sort_keep_the_elements_they_keep_shared() {
    let original = numbers();
    let mut copy = original.clone();
    copy.retain(|s| s != "two");
    copy.sort_by(|a, b| a.cmp(b));
    assert_eq!(copy.to_vec(), ["four", "one", "three", "zero"]);
    let original_addresses = original.addresses();
    assert!(
        copy.addresses()
            .iter()
            .all(|a| original_addresses.contains(a)),
        "no kept element was copied"
    );
}

#[test]
fn equality_compares_values_not_allocations() {
    let a = numbers();
    let b = numbers();
    assert_eq!(shared(&a, &b), 0);
    assert!(a == b);
    let mut c = a.clone();
    c.replace(2, "deux".to_string());
    assert!(a != c);
}

#[test]
fn a_spine_holds_exactly_its_elements() {
    // A large element with an `Arc`'s alignment, as a `Variable` has, is what
    // lets the standard library collect a `vec::IntoIter` into `Arc`s in the
    // source's buffer, which would keep 512 bytes of capacity per element.
    let big: Vec<[u64; 64]> = (0..100u64).map(|i| [i; 64]).collect();
    let mut shared_big = SharedVec::from(big);
    assert_eq!(shared_big.spine_capacity(), 100);
    shared_big.rewrite(|plain| plain.truncate(60));
    assert_eq!(shared_big.spine_capacity(), 60);
    let collected: SharedVec<[u64; 64]> = (0..40u64).map(|i| [i; 64]).collect();
    assert_eq!(collected.spine_capacity(), 40);
}
