// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export const baseURL = 'https://systemdynamics.net';

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

export interface SeriesProps {
  name: string;
  time: Float64Array;
  values: Float64Array;
}
export type Series = Readonly<SeriesProps>;
