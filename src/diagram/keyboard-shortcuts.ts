// Copyright 2026 The Simlin Authors. All rights reserved.
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

/**
 * Does this key event request a line break inside a multi-line text editor?
 *
 * Slate already inserts a break for Enter and Shift+Enter, but Cmd+Enter
 * (macOS "Apple enter") and Ctrl+Enter fall through both of its paths: on
 * modern browsers a modifier+Enter chord produces no `beforeinput` event at
 * all (so the insertParagraph/insertLineBreak path never fires), and the
 * legacy keydown fallback uses `is-hotkey`, which requires every modifier not
 * named in a binding to be absent, so metaKey/ctrlKey+Enter matches neither
 * `enter` (split block) nor `shift+enter` (soft break). Users reach for
 * Cmd+Enter to add a line to an equation, so editors that want it call this
 * and insert the break themselves.
 *
 * Ctrl+Enter is included so the gesture is consistent on non-mac keyboards. It
 * is safe: the global editor shortcuts (`detectUndoRedo`) bail out on editable
 * targets, and the equation/documentation fields carry no other Ctrl+Enter or
 * Cmd+Enter binding. Alt is excluded so Option/Alt combinations stay free for
 * platform text-navigation shortcuts.
 */
export function isNewlineChord(e: {
  key?: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
}): boolean {
  const isEnter = e.key === 'Enter' || e.code === 'Enter' || e.code === 'NumpadEnter';
  if (!isEnter || e.altKey) {
    return false;
  }
  return e.metaKey || e.ctrlKey;
}

export function detectUndoRedo(e: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}): UndoRedoAction {
  // Alt modifier is used for other shortcuts, don't trigger undo/redo
  if (e.altKey) {
    return null;
  }

  const isModifierPressed = e.metaKey || e.ctrlKey;
  const isZKey = e.key === 'z' || e.key === 'Z';
  const isYKey = e.key === 'y' || e.key === 'Y';

  // Ctrl+Y is the standard redo shortcut on Windows/Linux
  // (Cmd+Y on Mac is typically not redo, so we only check ctrlKey without metaKey)
  if (e.ctrlKey && !e.metaKey && !e.shiftKey && isYKey) {
    return 'redo';
  }

  if (!isModifierPressed || !isZKey) {
    return null;
  }

  return e.shiftKey ? 'redo' : 'undo';
}
