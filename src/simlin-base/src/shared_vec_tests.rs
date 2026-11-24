// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel::{ViewElement, view_element};

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

/// The pass the `update` rows run: an `o` after the first letter becomes `0`.
fn zeroed(s: &str) -> Option<String> {
    s[1..].contains('o').then(|| s.replace('o', "0"))
}

#[test]
fn update_replaces_what_the_pass_returns_and_keeps_the_rest_shared() {
    let original = numbers();
    let mut copy = original.clone();
    copy.update(|s| zeroed(s));
    assert_eq!(copy.to_vec(), ["zer0", "one", "tw0", "three", "f0ur"]);
    assert_eq!(original.to_vec(), ["zero", "one", "two", "three", "four"]);
    assert_eq!(shared(&original, &copy), 2);
}

#[test]
fn update_gives_one_result_whether_or_not_anything_shares_the_elements() {
    let mut alone = numbers();
    let untouched = alone.addresses();
    alone.update(|s| zeroed(s));
    let original = numbers();
    let mut shared_copy = original.clone();
    shared_copy.update(|s| zeroed(s));
    assert_eq!(alone.to_vec(), shared_copy.to_vec());
    // An element the pass returns nothing for stays where it was, in either.
    assert_eq!(alone.addresses()[1], untouched[1]);
    assert_eq!(shared_copy.addresses()[1], original.addresses()[1]);
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

fn cloud(uid: i32, x: f64) -> ViewElement {
    ViewElement::Cloud(view_element::Cloud {
        uid,
        flow_uid: 0,
        x,
        y: 0.0,
        compat: None,
    })
}

#[test]
fn share_identical_puts_back_the_allocation_of_each_element_kept_as_it_was() {
    let before: SharedVec<ViewElement> = vec![
        cloud(1, 1.0),
        cloud(2, 2.0),
        cloud(3, 0.0),
        cloud(4, f64::NAN),
        cloud(5, 5.0),
    ]
    .into();
    // Built afresh, as a layout or a host's replacement builds a view: one
    // element kept, one changed, one changed only in the sign of its zero,
    // one kept that holds a NaN, one new.
    let mut after: SharedVec<ViewElement> = vec![
        cloud(1, 1.0),
        cloud(2, 2.5),
        cloud(3, -0.0),
        cloud(4, f64::NAN),
        cloud(6, 5.0),
    ]
    .into();
    after.share_identical(&before, ViewElement::get_uid);
    let before_addresses = before.addresses();
    let kept: Vec<bool> = after
        .addresses()
        .iter()
        .map(|a| before_addresses.contains(a))
        .collect();
    assert_eq!(kept, [true, false, false, true, false]);
    assert!(
        matches!(&after[2], ViewElement::Cloud(c) if c.x.is_sign_negative()),
        "a -0.0 is not replaced by 0.0"
    );
}

#[test]
fn share_identical_compares_the_first_of_a_repeated_key() {
    let before: SharedVec<ViewElement> = vec![cloud(7, 1.0), cloud(7, 2.0)].into();
    let mut after: SharedVec<ViewElement> = vec![cloud(7, 1.0), cloud(7, 2.0)].into();
    after.share_identical(&before, ViewElement::get_uid);
    let b = before.addresses();
    let a = after.addresses();
    assert_eq!(a[0], b[0]);
    assert_ne!(a[1], b[1], "only the key's first element is compared");
}

#[test]
fn update_keeps_a_replacement_that_compares_equal() {
    // `0.0 == -0.0`, so a pass that returns one for the other has still
    // changed its element: what it returns is kept, never compared.
    let original: SharedVec<f64> = vec![0.0, 1.0].into();
    let mut copy = original.clone();
    copy.update(|x| (*x == 0.0).then_some(-0.0));
    assert!(copy[0].is_sign_negative(), "the replacement was kept");
    assert!(original[0].is_sign_positive(), "the original is untouched");
    assert_eq!(
        copy.addresses()[1],
        original.addresses()[1],
        "an element the pass left alone is still shared"
    );
}
