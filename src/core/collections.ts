// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { List, Map, Set } from 'immutable';

/**
 * Get the first element of a non-empty list.
 * Throws if the list is empty.
 */
export function first<T>(list: List<T>): T {
  const value = list.first();
  if (value === undefined) {
    throw new Error(`Expected non-empty list, got size=${list.size}`);
  }
  return value;
}

/**
 * Get the last element of a non-empty list.
 * Throws if the list is empty.
 */
export function last<T>(list: List<T>): T {
  const value = list.last();
  if (value === undefined) {
    throw new Error(`Expected non-empty list, got size=${list.size}`);
  }
  return value;
}

/**
 * Get element at index from a list.
 * Throws if the index is out of bounds.
 */
export function at<T>(list: List<T>, index: number): T {
  const value = list.get(index);
  if (value === undefined) {
    throw new Error(`Index ${index} out of bounds for list of size ${list.size}`);
  }
  return value;
}

/**
 * Get value from a map by key.
 * Throws if the key does not exist.
 */
export function getOrThrow<K, V>(map: Map<K, V>, key: K): V {
  const value = map.get(key);
  if (value === undefined) {
    throw new Error(`Missing key: ${key}`);
  }
  return value;
}

/**
 * Get the single element from a singleton set.
 * Throws if the set does not have exactly one element.
 */
export function only<T>(set: Set<T>): T {
  if (set.size !== 1) {
    throw new Error(`Expected singleton set, got size=${set.size}`);
  }
  return set.first() as T;
}
