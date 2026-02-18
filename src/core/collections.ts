// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Get the first element of a non-empty array.
 * Throws if the array is empty.
 */
export function first<T>(arr: readonly T[]): T {
  if (arr.length === 0) {
    throw new Error(`Expected non-empty array, got length=0`);
  }
  return arr[0];
}

/**
 * Get the last element of a non-empty array.
 * Throws if the array is empty.
 */
export function last<T>(arr: readonly T[]): T {
  if (arr.length === 0) {
    throw new Error(`Expected non-empty array, got length=0`);
  }
  return arr[arr.length - 1];
}

/**
 * Get element at index from an array.
 * Throws if the index is out of bounds.
 */
export function at<T>(arr: readonly T[], index: number): T {
  if (index < 0 || index >= arr.length) {
    throw new Error(`Index ${index} out of bounds for array of length ${arr.length}`);
  }
  return arr[index];
}

/**
 * Get value from a map by key.
 * Throws if the key does not exist.
 */
export function getOrThrow<K, V>(map: ReadonlyMap<K, V>, key: K): V {
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
export function only<T>(set: ReadonlySet<T>): T {
  if (set.size !== 1) {
    throw new Error(`Expected singleton set, got size=${set.size}`);
  }
  return set.values().next().value!;
}
