// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Keyboard scoping for Editor instances that share a document.
//
// The Editor's shortcuts (Delete/Backspace, Escape, undo/redo) are handled by a
// document-level keydown listener because the canvas is an <svg> that cannot
// hold focus (the Canvas focuses its container div after a click, but keys can
// still arrive with focus elsewhere). That is fine while one Editor owns the
// page (src/app, simlin-serve) and wrong the moment several share it (a
// notebook with an Editor per output cell): every instance would act on every
// key. Each instance therefore asks `editorOwnsKeyEvent` before acting.
//
// The decision is a pure function of the event's composed path and one piece
// of shared state -- which Editor root most recently saw pointer or focus
// activity inside it (`markActiveEditorRoot`). Focus alone is not enough to
// carry that state: focus falls to <body> whenever the focused control
// unmounts (a details panel closing on delete, the inline name editor
// committing), and users expect Ctrl+Z to keep working through those.

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
// One slot per PAGE: the Editor root that most recently saw a pointer press or
// focus enter it (or its React-tree portals). Shared state rather than a DOM
// attribute because "active" is a relation between instances, not a property
// of one; and page-global (a `globalThis` property under a registry symbol)
// rather than module-local because the instances sharing a page need not share
// a copy of this module: a notebook loads the widget bundle once per displayed
// widget (anywidget imports each from its own blob URL), so two Editors there
// hold two copies of this file, and with a module-local slot each copy keeps
// its own "last active" -- once both have been focused and focus falls to
// <body>, both claim the key and Delete/undo land on both models. The symbol
// comes from the realm-wide registry (`Symbol.for`), so every copy -- and
// every build, if two pysimlin versions ever sit on one page -- reaches the
// same slot; the slot's value is an Element or null and must stay that way, as
// the other copies reading it may be older code.

const ACTIVE_ROOT_SLOT = Symbol.for('@simlin/diagram:activeEditorRoot');

// `globalThis` viewed as the holder of that one symbol-keyed property.
const activeRootHost = globalThis as unknown as Record<typeof ACTIVE_ROOT_SLOT, Element | null | undefined>;

/** Record pointer/focus activity inside `root`. */
export function markActiveEditorRoot(root: Element): void {
  activeRootHost[ACTIVE_ROOT_SLOT] = root;
}

/** Forget `root` (on unmount) if it is the active one, so a key on <body> does
 *  not resolve to a dead instance -- or, worse, keep a live sibling from being
 *  the natural fallback until it is touched again. */
export function releaseEditorRoot(root: Element): void {
  if (activeRootHost[ACTIVE_ROOT_SLOT] === root) {
    activeRootHost[ACTIVE_ROOT_SLOT] = null;
  }
}

export function activeEditorRoot(): Element | null {
  return activeRootHost[ACTIVE_ROOT_SLOT] ?? null;
}
