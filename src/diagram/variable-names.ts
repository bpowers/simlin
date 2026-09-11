// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Naming rules for elements the editor creates or renames. The set of used
// idents is supplied by the controller: the committed model's variables plus
// the idents named on the rendered view, which carries every pending create and
// rename. Allocating against the committed model alone reused a default name
// while an earlier create was still in flight, and the engine's upsert then
// replaced that variable whatever its kind.

import { canonicalize } from '@simlin/core/canonicalize';

// The engine matches names canonically, so a collision is a canonical match.
// Allocation gives up after this many suffixes and returns the base name; the
// name commit then reports the collision instead of silently replacing.
const MaxNameSuffix = 1024;

/**
 * The first of `base`, `base 1`, `base 2`, ... whose canonical ident is not
 * in `used`; `base` itself when every candidate up to the cap is taken.
 */
export function allocateVariableName(base: string, used: ReadonlySet<string>): string {
  if (!used.has(canonicalize(base))) {
    return base;
  }
  for (let i = 1; i < MaxNameSuffix; i++) {
    const candidate = `${base} ${i}`;
    if (!used.has(canonicalize(candidate))) {
      return candidate;
    }
  }
  return base;
}

/**
 * The error to show when `newName` cannot name an element, or undefined when it
 * can. `currentIdent` is the ident the element already has (a rename), which
 * never collides with itself: a case-only rename changes the display spelling
 * of the same variable.
 */
export function nameCollisionError(
  newName: string,
  currentIdent: string | undefined,
  used: ReadonlySet<string>,
): string | undefined {
  const ident = canonicalize(newName);
  if (ident === currentIdent || !used.has(ident)) {
    return undefined;
  }
  return `A variable named '${newName}' already exists`;
}
