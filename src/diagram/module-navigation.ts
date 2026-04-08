// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

import type { Rect, UID } from '@simlin/core/datamodel';

/**
 * A single entry in the module navigation stack. Each entry stores
 * the model that was drilled INTO at that level, plus the PARENT's
 * selection and viewport state (to restore when navigating back).
 * An empty stack means the root model ('main') is active.
 */
export interface ModuleStackEntry {
  readonly modelName: string;
  readonly moduleIdent: string;
  readonly selection: ReadonlySet<UID>;
  readonly viewBox: Rect;
  readonly zoom: number;
}

export interface NavigateResult {
  readonly newStack: ReadonlyArray<ModuleStackEntry>;
  readonly restoredModelName: string;
  readonly restoredSelection: ReadonlySet<UID>;
  readonly restoredViewBox: Rect;
  readonly restoredZoom: number;
}

export interface BreadcrumbSegment {
  readonly label: string;
  readonly level: number;
}

export const STDLIB_MODEL_NAMES: ReadonlySet<string> = new Set([
  'delay1',
  'delay3',
  'npv',
  'smth1',
  'smth3',
  'systems_conversion',
  'systems_leak',
  'systems_rate',
  'trend',
]);

/**
 * Returns the model name from the top of the stack, or 'main' if empty.
 */
export function currentModelName(stack: ReadonlyArray<ModuleStackEntry>): string {
  if (stack.length === 0) {
    return 'main';
  }
  return stack[stack.length - 1].modelName;
}

/**
 * Creates a new stack entry capturing the current (parent) state and
 * appends it, making `targetModelName` the active model. Called during
 * drill-in: snapshot the current parent state, push it, and the new
 * top entry's `modelName` becomes the active model.
 */
export function pushModule(
  stack: ReadonlyArray<ModuleStackEntry>,
  targetModelName: string,
  moduleIdent: string,
  currentSelection: ReadonlySet<UID>,
  currentViewBox: Rect,
  currentZoom: number,
): ReadonlyArray<ModuleStackEntry> {
  const entry: ModuleStackEntry = {
    modelName: targetModelName,
    moduleIdent,
    selection: currentSelection,
    viewBox: currentViewBox,
    zoom: currentZoom,
  };
  return [...stack, entry];
}

/**
 * Removes the last entry and returns a NavigateResult with the restored
 * parent state. The restored model name comes from the entry below the
 * popped one, or 'main' if only one entry existed.
 *
 * Throws if stack is empty.
 */
export function popModule(stack: ReadonlyArray<ModuleStackEntry>): NavigateResult {
  if (stack.length === 0) {
    throw new Error('cannot pop from an empty module stack');
  }
  return navigateToLevel(stack, stack.length - 1);
}

/**
 * Truncates the stack to `targetLevel` entries, restoring state from
 * the entry at index `targetLevel`. Level 0 means "go back to main"
 * (empty stack). Called by breadcrumb clicks.
 *
 * - targetLevel is 0-indexed: 0 = main (root), 1 = first drill-in, etc.
 * - Current level = stack.length
 * - To navigate to level L: take the entry at index L (which stores
 *   level L's parent state), use it for restoration, and slice the
 *   stack to [0, L).
 *
 * Throws if targetLevel < 0, targetLevel >= stack.length, or stack is empty.
 */
export function navigateToLevel(stack: ReadonlyArray<ModuleStackEntry>, targetLevel: number): NavigateResult {
  if (stack.length === 0) {
    throw new Error('cannot navigate in an empty module stack');
  }
  if (targetLevel < 0) {
    throw new Error(`target level must be non-negative, got ${targetLevel}`);
  }
  if (targetLevel >= stack.length) {
    throw new Error(`target level ${targetLevel} is out of range for stack of length ${stack.length}`);
  }

  const poppedEntry = stack[targetLevel];
  const newStack = stack.slice(0, targetLevel);
  const restoredModelName = currentModelName(newStack);

  return {
    newStack,
    restoredModelName,
    restoredSelection: poppedEntry.selection,
    restoredViewBox: poppedEntry.viewBox,
    restoredZoom: poppedEntry.zoom,
  };
}

/**
 * Returns an array of breadcrumb segments for rendering. Always starts
 * with { label: 'main', level: 0 }, followed by one entry per stack
 * item using `moduleIdent` as the label.
 */
export function breadcrumbSegments(stack: ReadonlyArray<ModuleStackEntry>): ReadonlyArray<BreadcrumbSegment> {
  const segments: Array<BreadcrumbSegment> = [{ label: 'main', level: 0 }];
  for (let i = 0; i < stack.length; i++) {
    segments.push({ label: stack[i].moduleIdent, level: i + 1 });
  }
  return segments;
}

// Unicode TWO DOT PUNCTUATION used as a separator in stdlib model names
const STDLIB_PREFIX = 'stdlib\u{205A}';

/**
 * Returns true if the model name is one of the 9 stdlib models.
 * Handles both bare names (e.g. 'delay1') and engine-prefixed
 * names (e.g. 'stdlib⁚delay1').
 */
export function isStdlibModel(modelName: string): boolean {
  if (STDLIB_MODEL_NAMES.has(modelName)) {
    return true;
  }
  if (modelName.startsWith(STDLIB_PREFIX)) {
    return STDLIB_MODEL_NAMES.has(modelName.slice(STDLIB_PREFIX.length));
  }
  return false;
}
