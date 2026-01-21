// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export type UndoRedoAction = 'undo' | 'redo' | null;

export function detectUndoRedo(e: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
}): UndoRedoAction {
  const isModifierPressed = e.metaKey || e.ctrlKey;
  const isZKey = e.key === 'z' || e.key === 'Z';

  if (!isModifierPressed || !isZKey) {
    return null;
  }

  return e.shiftKey ? 'redo' : 'undo';
}
