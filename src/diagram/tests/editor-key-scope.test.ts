// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Table tests for the pure keyboard-scoping decision (editor-key-scope.ts).
// Every arm of `editorOwnsKeyEvent` is enumerated here; the two-instance
// integration (editor-keyboard-scoping.test.ts) exercises the same arms through
// real DOM events and the Editor's activity tracking.

import { describe, it, expect, beforeEach, rs } from '@rstest/core';

import {
  EDITOR_ROOT_ATTRIBUTE,
  activeEditorRoot,
  editorOwnsKeyEvent,
  isEditorRoot,
  markActiveEditorRoot,
  releaseEditorRoot,
} from '../editor-key-scope';

function makeRoot(): HTMLElement {
  const el = document.createElement('div');
  el.setAttribute(EDITOR_ROOT_ATTRIBUTE, '');
  document.body.appendChild(el);
  return el;
}

// The path a real keydown would carry from `target` outward, including the
// document and window like composedPath() does.
function pathFrom(target: Node): EventTarget[] {
  const path: EventTarget[] = [];
  for (let n: Node | null = target; n; n = n.parentNode) {
    path.push(n);
  }
  path.push(window);
  return path;
}

describe('editorOwnsKeyEvent', () => {
  let mine: HTMLElement;
  let other: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = '';
    mine = makeRoot();
    other = makeRoot();
  });

  it('arm 1: the target is inside my root -> mine, whoever was last active', () => {
    const inner = document.createElement('span');
    mine.appendChild(inner);
    expect(editorOwnsKeyEvent(mine, pathFrom(inner), other)).toBe(true);
    expect(editorOwnsKeyEvent(mine, pathFrom(inner), null)).toBe(true);
    expect(editorOwnsKeyEvent(mine, pathFrom(mine), null)).toBe(true);
  });

  it("arm 2: the target is inside another Editor's root -> not mine, even if I was last active", () => {
    const inner = document.createElement('span');
    other.appendChild(inner);
    expect(editorOwnsKeyEvent(mine, pathFrom(inner), mine)).toBe(false);
  });

  it('arm 2 (nesting): an Editor root nested inside mine claims events under it', () => {
    const nested = document.createElement('div');
    nested.setAttribute(EDITOR_ROOT_ATTRIBUTE, '');
    mine.appendChild(nested);
    const inner = document.createElement('span');
    nested.appendChild(inner);
    expect(editorOwnsKeyEvent(mine, pathFrom(inner), mine)).toBe(false);
    expect(editorOwnsKeyEvent(nested, pathFrom(inner), mine)).toBe(true);
  });

  it('arm 3: focus nowhere (<body>, <html>, or the bare document) -> the last-active root only', () => {
    for (const target of [document.body, document.documentElement, document]) {
      expect(editorOwnsKeyEvent(mine, pathFrom(target), mine)).toBe(true);
      expect(editorOwnsKeyEvent(mine, pathFrom(target), other)).toBe(false);
      expect(editorOwnsKeyEvent(mine, pathFrom(target), null)).toBe(false);
    }
  });

  it('arm 4: focus on some element outside every Editor -> nobody, last-active or not', () => {
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    expect(editorOwnsKeyEvent(mine, pathFrom(outside), mine)).toBe(false);
    expect(editorOwnsKeyEvent(mine, pathFrom(outside), null)).toBe(false);
  });
});

describe('the last-active tracker', () => {
  it('records the most recent mark and releases only the root it holds', () => {
    const a = makeRoot();
    const b = makeRoot();
    markActiveEditorRoot(a);
    expect(activeEditorRoot()).toBe(a);
    markActiveEditorRoot(b);
    expect(activeEditorRoot()).toBe(b);
    // Releasing a non-active root is a no-op: b's activity survives a's unmount.
    releaseEditorRoot(a);
    expect(activeEditorRoot()).toBe(b);
    releaseEditorRoot(b);
    expect(activeEditorRoot()).toBeNull();
  });
});

describe('the last-active tracker across module copies', () => {
  // A notebook loads the widget bundle -- and this module with it -- once per
  // displayed widget (anywidget imports each instance from its own blob URL),
  // so two Editors on one page hold two COPIES of this module. The slot must be
  // page-global or each copy keeps its own "last active" and a key on <body>
  // is claimed by both.
  it('two module copies share one slot: only the last-active root claims a <body> key', async () => {
    rs.resetModules();
    const copyA = await import('../editor-key-scope');
    rs.resetModules();
    const copyB = await import('../editor-key-scope');
    // Genuinely two evaluations of the module, not one namespace twice.
    expect(copyA.markActiveEditorRoot).not.toBe(copyB.markActiveEditorRoot);
    expect(copyA.markActiveEditorRoot).not.toBe(markActiveEditorRoot);

    const a = makeRoot();
    const b = makeRoot();
    // Each widget marks its own root through its own copy.
    copyA.markActiveEditorRoot(a);
    copyB.markActiveEditorRoot(b);
    // Every copy (this file's static import included) agrees on who is last.
    expect(copyA.activeEditorRoot()).toBe(b);
    expect(copyB.activeEditorRoot()).toBe(b);
    expect(activeEditorRoot()).toBe(b);
    // A key with focus on <body>: exactly one Editor owns it.
    const bodyPath = pathFrom(document.body);
    expect(copyA.editorOwnsKeyEvent(a, bodyPath, copyA.activeEditorRoot())).toBe(false);
    expect(copyB.editorOwnsKeyEvent(b, bodyPath, copyB.activeEditorRoot())).toBe(true);
    // Activity back in A flips it, seen from B's copy too.
    copyA.markActiveEditorRoot(a);
    expect(copyB.editorOwnsKeyEvent(b, bodyPath, copyB.activeEditorRoot())).toBe(false);
    expect(copyA.editorOwnsKeyEvent(a, bodyPath, copyA.activeEditorRoot())).toBe(true);
    // Unmounting A (its copy releases its root) leaves nobody active; B's
    // release of a root it does not hold is a no-op either way.
    copyB.releaseEditorRoot(b);
    expect(copyA.activeEditorRoot()).toBe(a);
    copyA.releaseEditorRoot(a);
    expect(copyB.activeEditorRoot()).toBeNull();
    expect(activeEditorRoot()).toBeNull();
  });
});

describe('isEditorRoot', () => {
  it('recognizes only elements carrying the root attribute', () => {
    expect(isEditorRoot(makeRoot())).toBe(true);
    expect(isEditorRoot(document.createElement('div'))).toBe(false);
    expect(isEditorRoot(document)).toBe(false);
    expect(isEditorRoot(window)).toBe(false);
    expect(isEditorRoot(null)).toBe(false);
  });
});
