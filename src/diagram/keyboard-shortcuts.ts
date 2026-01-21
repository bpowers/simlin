// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export type UndoRedoAction = 'undo' | 'redo' | null;

/**
 * Check if an element is an editable field where undo/redo should be handled
 * by the field itself rather than the global editor.
 */
export function isEditableElement(element: EventTarget | null): boolean {
  if (!element || !(element instanceof HTMLElement)) {
    return false;
  }

  const tagName = element.tagName.toUpperCase();
  if (tagName === 'INPUT' || tagName === 'TEXTAREA') {
    return true;
  }

  // Check both isContentEditable (which checks ancestors) and the contentEditable attribute
  if (element.isContentEditable || element.contentEditable === 'true') {
    return true;
  }

  return false;
}

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
