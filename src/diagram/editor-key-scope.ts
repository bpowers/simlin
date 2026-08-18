// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Keyboard scoping for Editor instances that share a document.
//
// The Editor's shortcuts (Delete/Backspace, Escape, undo/redo) are handled by a
// document-level keydown listener because the canvas is an <svg> that never
// holds focus: after a click the active element is blurred and key events
// target <body>. That is fine while one Editor owns the page (src/app,
// simlin-serve) and wrong the moment several share it (a notebook with an
// Editor per output cell): every instance would act on every key. Each
// instance therefore asks `editorOwnsKeyEvent` before acting.
//
// The decision is a pure function of the event's composed path and one piece
// of shared state -- which Editor root most recently saw pointer or focus
// activity inside it (`markActiveEditorRoot`). Focus alone is not enough to
// carry that state: besides the canvas blur, focus falls to <body> whenever the
// focused control unmounts (a details panel closing on delete, the inline name
// editor committing), and users expect Ctrl+Z to keep working through those.

/** Marks an Editor's outermost element so instances can recognize each other in
 *  an event path without a shared registry. Set with an empty value. */
export const EDITOR_ROOT_ATTRIBUTE = 'data-simlin-editor-root';

export function isEditorRoot(target: EventTarget | null | undefined): target is Element {
  return isElement(target) && target.hasAttribute(EDITOR_ROOT_ATTRIBUTE);
}

function isElement(target: EventTarget | null | undefined): target is Element {
  return typeof Element !== 'undefined' && target instanceof Element;
}

// <body> and <html> are where key events land when nothing has focus; they are
// not evidence that focus is "somewhere else on the page".
function isDocumentShell(el: Element): boolean {
  const tag = el.tagName.toUpperCase();
  return tag === 'BODY' || tag === 'HTML';
}

/**
 * Should the Editor rooted at `root` act on a key event whose
 * `composedPath()` is `path`? `lastActiveRoot` is the root that most recently
 * saw pointer/focus activity inside it (see `activeEditorRoot`), or null.
 *
 * Walking the path from the target outward, the first Editor root met decides:
 *  1. it is `root`             -> own it;
 *  2. it is another Editor     -> not ours (nested roots resolve to the inner one);
 * and when the path holds no Editor root at all:
 *  3. it holds no element besides <body>/<html> (focus is nowhere) -> ours iff
 *     this instance is the last-active one;
 *  4. it holds some other element (focus is on the host page) -> not ours.
 *
 * Callers apply the "never inside an editable element" rule before this one.
 */
export function editorOwnsKeyEvent(
  root: Element,
  path: ReadonlyArray<EventTarget>,
  lastActiveRoot: Element | null,
): boolean {
  let sawOutsideElement = false;
  for (const node of path) {
    if (node === root) {
      return true;
    }
    if (isEditorRoot(node)) {
      return false;
    }
    if (isElement(node) && !isDocumentShell(node)) {
      sawOutsideElement = true;
    }
  }
  if (sawOutsideElement) {
    return false;
  }
  return lastActiveRoot === root;
}

// ---- Shared last-active tracker -------------------------------------------
//
// One slot per JS realm: the Editor root that most recently saw a pointer
// press or focus enter it (or its React-tree portals). Module state rather
// than a DOM attribute because "active" is a relation between instances, not a
// property of one; a document-level owner would need the same singleton.

let activeRoot: Element | null = null;

/** Record pointer/focus activity inside `root`. */
export function markActiveEditorRoot(root: Element): void {
  activeRoot = root;
}

/** Forget `root` (on unmount) if it is the active one, so a key on <body> does
 *  not resolve to a dead instance -- or, worse, keep a live sibling from being
 *  the natural fallback until it is touched again. */
export function releaseEditorRoot(root: Element): void {
  if (activeRoot === root) {
    activeRoot = null;
  }
}

export function activeEditorRoot(): Element | null {
  return activeRoot;
}
