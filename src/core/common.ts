// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export const baseURL = 'https://app.simlin.com';

export function exists<T>(object: T | null): T {
  if (object === null) {
    throw new Error('expected non-null object');
  }
  return object;
}

export function defined<T>(object: T | undefined): T {
  if (object === undefined) {
    throw new Error('expected non-undefined object');
  }
  return object;
}

export function toInt(n: number): number {
  return n | 0;
}

export interface SeriesProps {
  readonly name: string;
  readonly time: Readonly<Float64Array>;
  readonly values: Readonly<Float64Array>;
}
export type Series = Readonly<SeriesProps>;

/** Create a new ReadonlyMap with one key updated. */
export function mapSet<K, V>(map: ReadonlyMap<K, V>, key: K, value: V): ReadonlyMap<K, V> {
  const m = new Map(map);
  m.set(key, value);
  return m;
}

/** Transform all values in a ReadonlyMap. */
export function mapValues<K, V>(map: ReadonlyMap<K, V>, fn: (value: V, key: K) => V): ReadonlyMap<K, V> {
  const result = new Map<K, V>();
  for (const [k, v] of map) {
    result.set(k, fn(v, k));
  }
  return result;
}

/** Check if two ReadonlySets contain the same elements. */
export function setsEqual<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  if (a.size !== b.size) return false;
  for (const item of a) {
    if (!b.has(item)) return false;
  }
  return true;
}

/** Create a new ReadonlySet with an element added. */
export function setAdd<T>(set: ReadonlySet<T>, value: T): ReadonlySet<T> {
  return new Set([...set, value]);
}

/** Create a new ReadonlySet with an element removed. */
export function setDelete<T>(set: ReadonlySet<T>, value: T): ReadonlySet<T> {
  const s = new Set(set);
  s.delete(value);
  return s;
}

/** Return a new array with element at index replaced. */
export function arrayWith<T>(arr: readonly T[], index: number, value: T): readonly T[] {
  const result = [...arr];
  result[index] = value;
  return result;
}

export function uint8ArraysEqual(a: Readonly<Uint8Array> | undefined, b: Readonly<Uint8Array> | undefined): boolean {
  if (a === undefined || b === undefined) {
    return a === b;
  }

  if (a.byteLength !== b.byteLength) {
    return false;
  }

  const len = a.length;
  for (let i = 0; i < len; i++) {
    if (a[i] !== b[i]) {
      return false;
    }
  }

  return true;
}
