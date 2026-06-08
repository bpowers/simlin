// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core -- pure functions for immutable reference array manipulation

import type { ModuleReference } from '@simlin/core/datamodel';

/**
 * Returns true if a reference with the same src and dst already exists.
 */
export function isDuplicateReference(references: ReadonlyArray<ModuleReference>, src: string, dst: string): boolean {
  return references.some((ref) => ref.src === src && ref.dst === dst);
}

/**
 * Add a new reference to the array. Returns a new array.
 * Does not add if src and dst are both non-empty and a duplicate already exists.
 * Allows duplicates when either src or dst is empty to support the
 * new-row placeholder pattern (user fills in via dropdowns).
 */
export function addReference(
  references: ReadonlyArray<ModuleReference>,
  src: string,
  dst: string,
): ReadonlyArray<ModuleReference> {
  if (src && dst && isDuplicateReference(references, src, dst)) {
    return references;
  }
  return [...references, { src, dst }];
}

/**
 * Remove the reference at the given index. Returns a new array.
 */
export function removeReference(
  references: ReadonlyArray<ModuleReference>,
  index: number,
): ReadonlyArray<ModuleReference> {
  return references.filter((_, i) => i !== index);
}

/**
 * Update the src of the reference at the given index. Returns a new array.
 */
export function updateReferenceSrc(
  references: ReadonlyArray<ModuleReference>,
  index: number,
  newSrc: string,
): ReadonlyArray<ModuleReference> {
  return references.map((ref, i) => (i === index ? { src: newSrc, dst: ref.dst } : ref));
}

/**
 * Update the dst of the reference at the given index. Returns a new array.
 */
export function updateReferenceDst(
  references: ReadonlyArray<ModuleReference>,
  index: number,
  newDst: string,
): ReadonlyArray<ModuleReference> {
  return references.map((ref, i) => (i === index ? { src: ref.src, dst: newDst } : ref));
}

/**
 * Get the list of available src variables from the parent model.
 * Returns variable idents that can serve as source wiring: stocks, flows, and auxes.
 * Excludes modules (they cannot be wired as inputs).
 */
export function getAvailableSrcVariables(
  parentVariables: ReadonlyMap<string, { type: string; ident: string }>,
): ReadonlyArray<string> {
  const result: Array<string> = [];
  for (const v of parentVariables.values()) {
    if (v.type === 'stock' || v.type === 'flow' || v.type === 'aux') {
      result.push(v.ident);
    }
  }
  return result.sort();
}
