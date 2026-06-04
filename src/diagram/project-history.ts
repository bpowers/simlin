// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Pure logic for the Editor's undo history.
 *
 * `projectHistory` is a newest-first array of serialized project snapshots;
 * `projectOffset` indexes the currently-displayed snapshot (0 = newest,
 * larger = further back in time). Undo increments the offset, redo
 * decrements it.
 */

export interface ProjectHistoryState {
  readonly projectHistory: readonly Readonly<Uint8Array>[];
  readonly projectOffset: number;
}

/**
 * Record a new snapshot as the head of the history.
 *
 * When the user has undone (offset > 0) and then edits, the entries at
 * indices [0, offset) were created *after* the snapshot the edit was made
 * from -- they are the abandoned redo branch, not ancestors of the new
 * state. Standard undo semantics discard them so the history stays a linear
 * ancestry of the current state; keeping them would make a later undo jump
 * to sibling states the user already navigated away from.
 */
export function advanceProjectHistory(
  current: ProjectHistoryState,
  snapshot: Readonly<Uint8Array>,
  maxSize: number,
): ProjectHistoryState {
  const ancestry = current.projectHistory.slice(current.projectOffset);
  return {
    projectHistory: [snapshot, ...ancestry].slice(0, maxSize),
    projectOffset: 0,
  };
}
