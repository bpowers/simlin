// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export { defined, exists, titleCase } from './common';

/// dName converts a string into the format the user
/// expects to see on a diagram.
export function dName(s: string): string {
  return s.replace(/\\n/g, '\n').replace(/_/g, ' ');
}

// swap the values at 2 indexes in the specified array, used for
// quicksort.
function swap<T>(array: T[], a: number, b: number): void {
  const tmp = array[a];
  array[a] = array[b];
  array[b] = tmp;
}

// partition used in quicksort, based off pseudocode
// on wikipedia
export function partition<T>(array: T[], cmp: Comparator<T>, l: number, r: number, p: number): number {
  const pValue = array[p];
  // move the pivot to the end
  swap(array, p, r);
  let store = l;
  for (let i = l; i < r; ++i) {
    if (cmp.lessThan(array[i], pValue)) {
      swap(array, i, store);
      store += 1;
    }
  }
  // move pivot to final location.
  swap(array, store, r);
  return store;
}

export interface Comparator<T> {
  lessThan(a: T, b: T): boolean;
}

/**
 *  Quicksort implementation, sorts in place.
 */
export function sort<T>(array: T[], cmp: Comparator<T>, l = 0, r = array.length - 1, part = partition): void {
  if (l >= r) {
    return;
  }

  const pivot = Math.floor(l + (r - l) / 2);
  const newPivot = part(array, cmp, l, r, pivot);
  sort(array, cmp, l, newPivot - 1, part);
  sort(array, cmp, newPivot + 1, r, part);
}

interface Table {
  x: number[];
  y: number[];
}

/**
 * Interpolates the y-value of the given index in the table.  If
 * the index is outside the range of the table, the minimum or
 * maximum value in the table is returned.
 *
 * @param table An object with x and y arrays.
 * @param index The requested index into the given table.
 * @return The y-value of the given index.
 */
export function lookup(table: Table, index: number): number {
  const size = table.x.length;
  if (size === 0) {
    return NaN;
  }

  const x = table.x;
  const y = table.y;

  if (index <= x[0]) {
    return y[0];
  } else if (index >= x[size - 1]) {
    return y[size - 1];
  }

  // binary search seems to be the most appropriate choice here.
  let low = 0;
  let high = size;
  let mid: number;
  while (low < high) {
    mid = Math.floor(low + (high - low) / 2);
    if (x[mid] < index) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }

  const i = low;
  if (x[i] === index) {
    return y[i];
  } else {
    // slope = deltaY/deltaX
    const slope = (y[i] - y[i - 1]) / (x[i] - x[i - 1]);
    // y = m*x + b
    return (index - x[i - 1]) * slope + y[i - 1];
  }
}

/**
 *  Returns the minimum of either of the arguments
 */
export function min(a: number, b: number): number {
  return a < b ? a : b;
}

/**
 * numArr returns a new array, composed of the result of calling
 * parseFloat on every item in arr.
 */
export function numArr(arr: any[]): number[] {
  return arr.map(parseFloat);
}

// eslint-disable-next-line @typescript-eslint/explicit-module-boundary-types
export function floatAttr(o: any, n: string): number {
  // eslint-disable-next-line @typescript-eslint/no-unsafe-call
  return parseFloat(o.getAttribute(n));
}

// wrapper/re-implementation of querySelector that works under
// Node with xmldom.
// eslint-disable-next-line @typescript-eslint/explicit-module-boundary-types
export function qs(e: any, s: string): any {
  if (e.querySelector) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-return
    return e.querySelector(s);
  }

  const selectors = s.split('>');
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  let curr = e;

  outer: for (let i = 0; curr && i < selectors.length; i++) {
    for (const n of curr.childnodes) {
      if (!n.tagName) {
        continue;
      }
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call
      if (n.tagName.toLowerCase() === selectors[i].toLowerCase()) {
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
        curr = n;
        continue outer;
      }
    }
    curr = null;
  }
  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return curr;
}

// eslint-disable-next-line @typescript-eslint/explicit-module-boundary-types
export function querySelectorInner(e: any, selectors: string[]): any {
  const sel = selectors[0];
  const rest = selectors.slice(1);
  let result: any[] = [];
  for (const child of e.childNodes) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-call
    if (child.tagName && child.tagName.toLowerCase() === sel) {
      if (rest.length) {
        result = result.concat(querySelectorInner(child, rest));
      } else {
        result.push(child);
      }
    }
  }
  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return result;
}

// wrapper/re-implementation of querySelectorAll that works under
// Node with xmldom
// eslint-disable-next-line @typescript-eslint/explicit-module-boundary-types
export function qsa(e: any, s: string): any[] {
  if (e.querySelectorAll) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return,@typescript-eslint/no-unsafe-call
    return e.querySelectorAll(s);
  }
  const selectors = s.split('>').map((sel: string): string => {
    return sel.toLowerCase();
  });

  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return querySelectorInner(e, selectors);
}

export function isNaN(n: number): boolean {
  // eslint-disable-next-line no-self-compare
  return n !== n;
}
