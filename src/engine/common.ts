// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Map, Set } from 'immutable';

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

export function titleCase(str: string): string {
  return str.replace(/(?:^|\s)\w/g, (match: string): string => {
    return match.toUpperCase();
  });
}

export class Error {
  static Version: Error = new Error('bad xml or unknown smile version');
  static BadTime: Error = new Error('bad time (control) data');

  readonly name = 'sd.js error';
  readonly message: string;

  constructor(msg: string) {
    this.message = msg;
  }
}

export interface Properties {
  usesTime?: boolean;
}

// whether identifiers are a builtin.  Implementation is in
// Builtin module in runtime_src.js
export const builtins: Map<string, Properties> = Map({
  abs: {},
  arccos: {},
  arcsin: {},
  arctan: {},
  cos: {},
  exp: {},
  inf: {},
  int: {},
  ln: {},
  log10: {},
  lookup: {},
  max: {},
  min: {},
  pi: {},
  pulse: {
    usesTime: true,
  },
  sin: {},
  sqrt: {},
  safediv: {},
  tan: {},
});

export const reserved: Set<string> = Set<string>(['if', 'then', 'else']);

export const canonicalize = (id: string): string => {
  let quoted = false;
  if (id.length > 1) {
    const f = id.slice(0, 1);
    const l = id.slice(id.length - 1);
    quoted = f === '"' && l === '"';
  }
  id = id.toLowerCase();
  id = id.replace(/\\n/g, '_');
  id = id.replace(/\\\\/g, '\\');
  id = id.replace(/\\"/g, '\\');
  id = id.replace(/[_\r\n\t \xa0]+/g, '_');
  if (quoted) {
    return id.slice(1, -1);
  }
  return id;
};
