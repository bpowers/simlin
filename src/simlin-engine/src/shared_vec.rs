// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! [`SharedVec`], the vector a model's variables and a view's elements are
//! held in, so a copy of a project shares every one of them until it is edited.
//!
//! A host copies its project for every undo step. With the variables and view
//! elements behind one `Arc` each, a copy costs a pointer per element, and an
//! edit copies only the elements it changes. Sharing survives only as long as
//! nothing touches an element it doesn't change, so the vector has no
//! `iter_mut`, no `IndexMut` and no `&mut` iteration: each mutation names what
//! it touches.
//!
//! - One element: [`SharedVec::get_mut`], [`SharedVec::find_mut`], or
//!   [`SharedVec::replace`], which puts a new value in place without copying
//!   the old one.
//! - The elements a test picks: [`SharedVec::edit_where`].
//! - A pass that can't tell beforehand which elements it changes (an equation
//!   rewrite has to parse to know): [`SharedVec::edit_each`], which edits a
//!   copy and keeps it only if it differs.
//! - A pass that changes every element, on a vector nothing shares yet (an
//!   import, a generated layout): [`SharedVec::rewrite`], over a plain `Vec`.
//!
//! A vector built afresh in place of one it replaces (a view laid out again,
//! or a host's whole replacement view) shares nothing it kept until
//! [`SharedVec::share_identical`] puts the old allocation back wherever the
//! element is [`Identical`] to the one it replaces.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

/// Equality bit for bit: whether one value can stand in for another behind
/// one allocation.
///
/// `PartialEq` compares floats by value, so `0.0 == -0.0` although the two
/// save differently. A kept element must be indistinguishable from the one it
/// stands in for, so sharing compares with this, never with `==`.
pub trait Identical {
    fn identical(&self, other: &Self) -> bool;
}

impl Identical for f64 {
    fn identical(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

/// Types whose `==` already compares every bit.
macro_rules! identical_by_eq {
    ($($t:ty),* $(,)?) => {
        $(impl Identical for $t {
            fn identical(&self, other: &Self) -> bool {
                self == other
            }
        })*
    };
}
identical_by_eq!(bool, i32, u32, String);

impl<T: Identical> Identical for Option<T> {
    fn identical(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.identical(b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T: Identical> Identical for Vec<T> {
    fn identical(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.identical(b))
    }
}

impl<A: Identical, B: Identical> Identical for (A, B) {
    fn identical(&self, other: &Self) -> bool {
        self.0.identical(&other.0) && self.1.identical(&other.1)
    }
}

/// Implements [`Identical`] for a struct field by field. The struct is
/// destructured without `..`, so a field added to it fails to compile until
/// it is listed here and compared.
macro_rules! identical_fields {
    ($t:ident { $($field:ident),* $(,)? }) => {
        impl $crate::shared_vec::Identical for $t {
            fn identical(&self, other: &Self) -> bool {
                let $t { $($field),* } = self;
                true $(&& $crate::shared_vec::Identical::identical($field, &other.$field))*
            }
        }
    };
}
pub(crate) use identical_fields;

/// A vector whose elements are shared with every clone of it until one of
/// them is edited.
#[derive(Clone, PartialEq)]
pub struct SharedVec<T>(Vec<Arc<T>>);

impl<T> Default for SharedVec<T> {
    fn default() -> Self {
        SharedVec(Vec::new())
    }
}

#[cfg(feature = "debug-derive")]
impl<T: std::fmt::Debug + Clone> std::fmt::Debug for SharedVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Clone> SharedVec<T> {
    pub fn new() -> Self {
        SharedVec(Vec::new())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator + Clone {
        self.0.iter().map(|e| &**e)
    }
    pub fn first(&self) -> Option<&T> {
        self.0.first().map(|e| &**e)
    }
    pub fn last(&self) -> Option<&T> {
        self.0.last().map(|e| &**e)
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        self.0.get(index).map(|e| &**e)
    }
    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|e| e == value)
    }
    /// Every element, copied out of the sharing.
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// The element at `index`, mutably: it alone is copied out of the sharing.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.0.get_mut(index).map(Arc::make_mut)
    }
    /// The first element `found` picks, mutably: it alone is copied out of the
    /// sharing.
    pub fn find_mut(&mut self, found: impl FnMut(&T) -> bool) -> Option<&mut T> {
        let index = self.iter().position(found)?;
        self.get_mut(index)
    }
    /// Puts `value` in place of the element at `index`, which is released
    /// rather than copied. Panics if `index` is out of bounds.
    pub fn replace(&mut self, index: usize, value: T) {
        self.0[index] = Arc::new(value);
    }
    /// Changes each element `needs` picks through `edit`, copying only those
    /// out of the sharing.
    pub fn edit_where(&mut self, mut needs: impl FnMut(&T) -> bool, mut edit: impl FnMut(&mut T)) {
        for element in &mut self.0 {
            if needs(element) {
                edit(Arc::make_mut(element));
            }
        }
    }
    /// Changes every element through `edit`, keeping shared each one the edit
    /// leaves as it was. An element nothing else holds is edited in place; a
    /// shared one is edited in a copy, which replaces it only if the two
    /// differ.
    pub fn edit_each(&mut self, mut edit: impl FnMut(&mut T))
    where
        T: PartialEq,
    {
        for element in &mut self.0 {
            if let Some(unique) = Arc::get_mut(element) {
                edit(unique);
                continue;
            }
            let mut copy = T::clone(element);
            edit(&mut copy);
            if copy != **element {
                *element = Arc::new(copy);
            }
        }
    }
    /// Puts `from`'s allocation in place of each element here that is
    /// identical to `from`'s element with the same `key`, so a vector built
    /// afresh shares everything it kept with the one it replaces. Where `from`
    /// holds a key twice, its first element is the one compared.
    pub fn share_identical<K: Eq + Hash>(&mut self, from: &SharedVec<T>, key: impl Fn(&T) -> K)
    where
        T: Identical,
    {
        let mut by_key: HashMap<K, &Arc<T>> = HashMap::with_capacity(from.len());
        for element in &from.0 {
            by_key.entry(key(element)).or_insert(element);
        }
        for element in &mut self.0 {
            if let Some(old) = by_key.get(&key(element))
                && !Arc::ptr_eq(old, element)
                && old.identical(element)
            {
                *element = Arc::clone(old);
            }
        }
    }
    /// Runs `f` over the elements as a plain vector, for a pass that changes
    /// them all: each is copied out of the sharing first (moved, if nothing
    /// else holds it), and the result shares nothing.
    pub fn rewrite<R>(&mut self, f: impl FnOnce(&mut Vec<T>) -> R) -> R {
        let mut plain: Vec<T> = std::mem::take(&mut self.0)
            .into_iter()
            .map(Arc::unwrap_or_clone)
            .collect();
        let result = f(&mut plain);
        self.0 = arc_spine(plain);
        result
    }

    pub fn push(&mut self, value: T) {
        self.0.push(Arc::new(value))
    }
    pub fn insert(&mut self, index: usize, value: T) {
        self.0.insert(index, Arc::new(value))
    }
    pub fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        self.0.extend(values.into_iter().map(Arc::new))
    }
    pub fn append(&mut self, other: &mut SharedVec<T>) {
        self.0.append(&mut other.0)
    }
    pub fn remove(&mut self, index: usize) -> T {
        Arc::unwrap_or_clone(self.0.remove(index))
    }
    pub fn pop(&mut self) -> Option<T> {
        self.0.pop().map(Arc::unwrap_or_clone)
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.0.retain(|e| keep(e))
    }
    pub fn clear(&mut self) {
        self.0.clear()
    }
    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len)
    }
    pub fn reverse(&mut self) {
        self.0.reverse()
    }
    pub fn sort_by(&mut self, mut compare: impl FnMut(&T, &T) -> std::cmp::Ordering) {
        self.0.sort_by(|a, b| compare(a, b))
    }
    pub fn sort_by_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        self.0.sort_by_key(|e| key(e))
    }
    pub fn sort_by_cached_key<K: Ord>(&mut self, mut key: impl FnMut(&T) -> K) {
        self.0.sort_by_cached_key(|e| key(e))
    }
}

impl<T: Clone> std::ops::Index<usize> for SharedVec<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.0[index]
    }
}

/// `elements`, each behind its own `Arc`, in a spine of exactly their number.
///
/// Never `collect` a `vec::IntoIter<T>` mapped to `Arc<T>`: the standard
/// library reuses the source vector's buffer for the result when the element
/// shrinks, so the spine would keep `size_of::<T>()` bytes of capacity per
/// element (632 for a `Variable`) and cost as much as the elements it points
/// to.
fn arc_spine<T>(elements: impl IntoIterator<Item = T>) -> Vec<Arc<T>> {
    let elements = elements.into_iter();
    let mut spine = Vec::with_capacity(elements.size_hint().0);
    spine.extend(elements.map(Arc::new));
    spine.shrink_to_fit();
    spine
}

impl<T> FromIterator<T> for SharedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        SharedVec(arc_spine(iter))
    }
}

impl<T> From<Vec<T>> for SharedVec<T> {
    fn from(values: Vec<T>) -> Self {
        values.into_iter().collect()
    }
}

impl<T: Clone> From<SharedVec<T>> for Vec<T> {
    fn from(values: SharedVec<T>) -> Self {
        values.into_iter().collect()
    }
}

impl<'a, T: Clone> IntoIterator for &'a SharedVec<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Arc<T>>, fn(&'a Arc<T>) -> &'a T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|e| &**e)
    }
}

impl<T: Clone> IntoIterator for SharedVec<T> {
    type Item = T;
    type IntoIter = std::iter::Map<std::vec::IntoIter<Arc<T>>, fn(Arc<T>) -> T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter().map(Arc::unwrap_or_clone)
    }
}

#[cfg(test)]
impl<T> SharedVec<T> {
    /// Each element's allocation, by position: two vectors share an element
    /// where they hold the same address.
    pub(crate) fn addresses(&self) -> Vec<usize> {
        self.0.iter().map(|e| Arc::as_ptr(e) as usize).collect()
    }

    /// The spine's capacity, which `arc_spine` keeps to its length.
    pub(crate) fn spine_capacity(&self) -> usize {
        self.0.capacity()
    }
}

#[cfg(test)]
#[path = "shared_vec_tests.rs"]
mod tests;
