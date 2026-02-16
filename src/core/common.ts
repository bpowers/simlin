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
