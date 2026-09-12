// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A live gesture's lifecycle, driven through real pointer events: what aborts it
// (E5: pointercancel, a lost release, a second pointer, a pinch, a token change,
// a geometry change under it), what does not (a republish that changes nothing
// it reads, a selection change), and that no path throws or leaves gesture state
// behind for the next press. Rows port the audit's lifecycle probes (L1-L8) and
// press defects (H3, H4, M3, M8, L-a, L-b, L-c, P-1, P-3, P-4, P-5).
//
// What this does not establish: the geometry a gesture commits
// (gesture-planner*.test.ts) or the controller refusing a stale token
// (project-controller.test.ts).

import { describe, it, expect, rs } from '@rstest/core';

import { act } from '@testing-library/react';

import type { Model, StockFlowView, Variable, ViewElement } from '@simlin/core/datamodel';

import {
  dispatchWheel,
  makeAux,
  makeCloud,
  makeFlow,
  makeLink,
  makeStock,
  pointerCancel,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
  type CanvasHarness,
} from './canvas-gesture-harness';

const B1 = { buttons: 1 };

function stockCloud(): ViewElement[] {
  return [
    makeStock(1, 'stock', 100, 100),
    makeCloud(2, 3, 300, 100),
    makeFlow(
      3,
      'flow',
      [
        { x: 122.5, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: 2 },
      ],
      { x: 200, y: 100 },
    ),
  ];
}

function linkScene(): ViewElement[] {
  return [makeAux(1, 'a', 100, 100), makeAux(2, 'b', 300, 100), makeAux(3, 'c', 300, 300), makeLink(4, 1, 2)];
}

function viewOf(h: CanvasHarness, elements: readonly ViewElement[]): StockFlowView {
  return { ...h.view(), elements };
}

const auxX = (h: CanvasHarness, i = 0): string | null => h.queryAll('g.simlin-aux circle')[i].getAttribute('cx');

// The live viewport the content group is drawn with: offset and zoom.
function translate(transform: string | null): { x: number; y: number; zoom: number } {
  const m = /matrix\(([^)]+)\)/.exec(transform ?? '');
  if (!m) {
    throw new Error(`no matrix in transform: ${transform}`);
  }
  const [a, , , , e, f] = m[1].split(/[\s,]+/).map(Number);
  return { x: e / a, y: f / a, zoom: a };
}

function captureWindowErrors(): { errors: unknown[]; stop: () => void } {
  const errors: unknown[] = [];
  const onError = (e: ErrorEvent): void => {
    errors.push(e.error ?? e.message);
    e.preventDefault();
  };
  window.addEventListener('error', onError);
  return { errors, stop: () => window.removeEventListener('error', onError) };
}

describe('Canvas gesture lifecycle: aborts (E5)', () => {
  it('M3/L3: a pointercancel mid link reattach commits and deletes nothing', () => {
    const h = renderCanvas({ elements: linkScene() });
    h.clearMountCalls();
    pointerDown(h.query('path.simlin-arrowhead-link')!, 289, 100);
    pointerMove(h.svg, 300, 300, B1);
    pointerCancel(h.svg, 300, 300);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    expect(h.query('path.simlin-arrowhead-link')).not.toBeNull();
  });

  it('L4: a pointercancel mid flow creation leaves no flow, and mid aux creation opens no editor', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();
    pointerDown(h.svg, 200, 200);
    pointerMove(h.svg, 300, 200, B1);
    expect(h.query('g.simlin-flow')).not.toBeNull();
    pointerCancel(h.svg, 300, 200);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.query('g.simlin-flow')).toBeNull();

    const h2 = renderCanvas({ elements: [], selectedTool: 'aux' });
    h2.clearMountCalls();
    pointerDown(h2.svg, 200, 200);
    pointerMove(h2.svg, 260, 240, B1);
    pointerCancel(h2.svg, 260, 240);
    expect(h2.query('[contenteditable]')).toBeNull();
    expect(h2.query('g.simlin-aux')).toBeNull();
  });

  it.each([1, 0])(
    'M8/L5: a mouse move with no button held (a lost release) cancels, for pointerId %d alike',
    (pointerId) => {
      const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
      h.clearMountCalls();
      pointerDown(h.query('g.simlin-aux')!, 100, 100, { pointerId });
      pointerMove(h.svg, 150, 150, { pointerId, buttons: 1 });
      pointerMove(h.svg, 160, 160, { pointerId, buttons: 0 });
      expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
      expect(auxX(h)).toBe('100');
      // Later hover moves move nothing.
      pointerMove(h.svg, 250, 250, { pointerId, buttons: 0 });
      pointerUp(h.svg, 250, 250, { pointerId });
      expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
      expect(auxX(h)).toBe('100');
    },
  );

  it('P-1: a second pointer while dragging aborts the drag', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100, { pointerId: 1 });
    pointerMove(h.svg, 160, 160, { pointerId: 1, buttons: 1 });
    pointerDown(h.svg, 500, 500, { pointerId: 2, pointerType: 'pen', isPrimary: false });
    expect(auxX(h)).toBe('100');
    pointerUp(h.svg, 160, 160, { pointerId: 1 });
    pointerUp(h.svg, 500, 500, { pointerId: 2, pointerType: 'pen', isPrimary: false });
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('L-a/L7: a pinch during flow creation leaves no phantom flow after both fingers lift', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();
    const t1 = { pointerId: 1, pointerType: 'touch' };
    const t2 = { pointerId: 2, pointerType: 'touch', isPrimary: false };
    pointerDown(h.svg, 200, 200, t1);
    pointerMove(h.svg, 300, 200, { ...t1, buttons: 1 });
    expect(h.queryAll('g.simlin-flow')).toHaveLength(1);
    pointerDown(h.svg, 500, 500, t2);
    pointerMove(h.svg, 550, 550, { ...t2, buttons: 1 });
    pointerUp(h.svg, 550, 550, t2);
    pointerUp(h.svg, 300, 200, t1);
    expect(h.queryAll('g.simlin-flow')).toHaveLength(0);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('P-3: an undo landing mid drag (the token moves) aborts it', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 160, 160, B1);
    expect(auxX(h)).toBe('160');
    h.setProps({ token: 1 });
    expect(auxX(h)).toBe('100');
    pointerMove(h.svg, 180, 180, B1);
    pointerUp(h.svg, 180, 180);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(auxX(h)).toBe('100');
  });

  it('L2: deleting the dragged flow mid sink drag neither throws nor commits', () => {
    const h = renderCanvas({ elements: stockCloud() });
    h.clearMountCalls();
    pointerDown(h.query('path.simlin-arrowhead-flow')!, 290, 100);
    pointerMove(h.svg, 350, 150, B1);
    const cap = captureWindowErrors();
    try {
      h.setProps({ selection: new Set(), view: viewOf(h, [makeStock(1, 'stock', 100, 100)]) });
      pointerMove(h.svg, 360, 160, B1);
      pointerUp(h.svg, 360, 160);
    } finally {
      cap.stop();
    }
    expect(cap.errors).toEqual([]);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.query('g.simlin-flow')).toBeNull();
  });
});

describe('Canvas gesture lifecycle: what keeps a gesture live', () => {
  it('H3/L1: clearing the selection mid link-arrowhead drag does not crash, and the drag still commits', () => {
    const h = renderCanvas({ elements: linkScene() });
    h.clearMountCalls();
    pointerDown(h.query('path.simlin-arrowhead-link')!, 289, 100);
    pointerMove(h.svg, 300, 200, B1);
    const cap = captureWindowErrors();
    try {
      h.setProps({ selection: new Set() });
      pointerMove(h.svg, 300, 300, B1);
      pointerUp(h.svg, 300, 300);
    } finally {
      cap.stop();
    }
    expect(cap.errors).toEqual([]);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });

  it('E5: a republish that changes nothing the gesture reads keeps it', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 160, 160, B1);
    // A fresh view object whose elements carry annotations and float noise only.
    const republished = viewOf(
      h,
      h.view().elements.map((el) => ({ ...el, x: el.x + 1e-9, var: undefined }) as ViewElement),
    );
    h.setProps({ view: { ...republished, nextUid: 999 } });
    expect(Number(auxX(h))).toBeCloseTo(160, 6);
    pointerUp(h.svg, 160, 160);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });

  // The controller republishes a new project for each of these: a new view object
  // with fresh element objects, and a new model. None moves any geometry by more
  // than GEOMETRY_EPSILON, so the drag continues and commits.
  const ulp = (v: number): number => v + Math.max(Number.MIN_VALUE, Math.abs(v) * Number.EPSILON);
  const copies = (view: StockFlowView): StockFlowView => ({
    ...view,
    elements: view.elements.map((el) => ({ ...el }) as ViewElement),
  });
  // Every variable of `model` with `fields` attached, as the controller's render
  // annotations attach sim series and errors.
  const annotated = (model: Model, fields: object): Model => ({
    ...model,
    variables: new Map([...model.variables].map(([ident, v]) => [ident, { ...v, ...fields } as Variable])),
  });
  const BENIGN: ReadonlyArray<{ name: string; annotates: boolean; republish: (h: CanvasHarness) => void }> = [
    {
      name: 'a pending edit landing (one ULP of drift; element objects and nextUid re-derived)',
      annotates: false,
      republish: (h) =>
        h.setProps({
          view: {
            ...h.view(),
            nextUid: h.view().nextUid + 7,
            elements: h.view().elements.map((el) => ({ ...el, x: ulp(el.x), y: ulp(el.y) }) as ViewElement),
          },
        }),
    },
    {
      name: 'sim results attaching',
      annotates: true,
      republish: (h) =>
        h.setProps({
          view: copies(h.view()),
          model: annotated(h.model(), {
            data: [{ name: 'b', time: new Float64Array([0, 1, 2]), values: new Float64Array([1, 3, 2]) }],
          }),
        }),
    },
    {
      name: 'error annotations updating',
      annotates: true,
      republish: (h) =>
        h.setProps({
          view: copies(h.view()),
          model: annotated(h.model(), { errors: [{ start: 0, end: 1, code: 'unknown_dependency' }] }),
        }),
    },
  ];
  for (const row of BENIGN) {
    it(`E5: ${row.name} keeps a live drag, which then commits`, () => {
      const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100), makeAux(11, 'b', 400, 400)] });
      h.clearMountCalls();
      pointerDown(h.query('g.simlin-aux')!, 100, 100);
      pointerMove(h.svg, 160, 160, B1);
      const bBefore = h.queryAll('g.simlin-aux')[1].outerHTML;
      row.republish(h);
      if (row.annotates) {
        // The annotation reached the render (a sparkline or a warning dot on b).
        expect(h.queryAll('g.simlin-aux')[1].outerHTML).not.toBe(bBefore);
      }
      expect(Number(auxX(h))).toBeCloseTo(160, 6);
      pointerMove(h.svg, 170, 170, B1);
      pointerUp(h.svg, 170, 170);
      expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
    });
  }

  it('E5: a real change to an element the drag does not read still aborts it (the comparison is whole-view)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100), makeAux(11, 'b', 400, 400)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 160, 160, B1);
    h.setProps({
      view: viewOf(
        h,
        h.view().elements.map((el) => (el.uid === 11 ? ({ ...el, x: el.x + 20 } as ViewElement) : el)),
      ),
    });
    expect(Number(auxX(h))).toBeCloseTo(100, 6);
    pointerMove(h.svg, 170, 170, B1);
    pointerUp(h.svg, 170, 170);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('Escape cancels a live drag: the preview returns to the view, the release commits nothing, the next press starts fresh', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100), makeAux(11, 'b', 400, 400)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 160, 160, B1);
    expect(Number(auxX(h))).toBeCloseTo(160, 6);
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    expect(auxX(h)).toBe('100');
    pointerMove(h.svg, 180, 180, B1);
    expect(auxX(h)).toBe('100');
    pointerUp(h.svg, 180, 180);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    // Nothing is left live for the next press to abort against: a band selects b.
    pointerDown(h.svg, 380, 380);
    pointerMove(h.svg, 420, 420, B1);
    pointerUp(h.svg, 420, 420);
    expect([...h.selection()]).toEqual([11]);
  });

  it('M-1: a link a->b landing while c is dragged keeps the drag, which commits', () => {
    const h = renderCanvas({
      elements: [makeAux(1, 'a', 100, 100), makeAux(2, 'b', 300, 100), makeAux(3, 'c', 300, 300)],
      selectedTool: 'link',
    });
    h.clearMountCalls();
    pointerDown(h.queryAll('g.simlin-aux')[0], 100, 100);
    pointerMove(h.svg, 300, 100, B1);
    pointerUp(h.svg, 300, 100);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
    h.setProps({ selectedTool: undefined });
    h.callbacks.onCommitGesture.mockClear();

    pointerDown(h.queryAll('g.simlin-aux')[2], 300, 300);
    pointerMove(h.svg, 340, 340, B1);
    // The link's patch lands: the datamodel reads a link's position back as NaN
    // and re-derives isStraight.
    h.setProps({
      view: viewOf(
        h,
        h
          .view()
          .elements.map((el) =>
            el.type === 'link' ? ({ ...el, x: NaN, y: NaN, isStraight: !el.isStraight } as ViewElement) : el,
          ),
      ),
    });
    pointerMove(h.svg, 350, 350, B1);
    expect(Number(auxX(h, 2))).toBeCloseTo(350, 6);
    pointerUp(h.svg, 350, 350);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });

  it('S-1: a lost release forgets its pointer, so a later single touch pans instead of pinching', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100, { pointerId: 1 });
    pointerMove(h.svg, 150, 150, { pointerId: 1, buttons: 0 });
    const before = translate(h.getTransform());
    pointerDown(h.svg, 500, 500, { pointerId: 2, pointerType: 'touch', isPrimary: true });
    pointerMove(h.svg, 540, 560, { pointerId: 2, pointerType: 'touch', isPrimary: true, buttons: 1 });
    const after = translate(h.getTransform());
    expect(after.zoom).toBe(before.zoom);
    expect({ x: after.x - before.x, y: after.y - before.y }).toEqual({ x: 40, y: 60 });
    pointerUp(h.svg, 540, 560, { pointerId: 2, pointerType: 'touch', isPrimary: true });
  });

  it('S-7: a press captures the pointer on the svg root, not on the pressed element', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    const rootCapture = rs.fn();
    const elementCapture = rs.fn();
    const aux = h.query('g.simlin-aux')!;
    h.svg.setPointerCapture = rootCapture;
    aux.setPointerCapture = elementCapture;
    pointerDown(aux, 100, 100, { pointerId: 7 });
    expect(rootCapture).toHaveBeenCalledWith(7);
    expect(elementCapture).not.toHaveBeenCalled();
    pointerUp(h.svg, 100, 100, { pointerId: 7 });
  });

  it('P-4: a wheel pan during an element drag keeps the element under the pointer and commits once', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 160, 160, B1);
    dispatchWheel(h.svg, { deltaY: 40, clientX: 160, clientY: 160 });
    pointerMove(h.svg, 160, 160, B1);
    pointerUp(h.svg, 160, 160);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });
});

describe('Canvas gesture lifecycle: nothing is left behind', () => {
  it('H4/L6: a commit that throws ends the gesture, so the next press starts a rubber band', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100), makeAux(11, 'b', 400, 400)] });
    h.clearMountCalls();
    h.callbacks.onCommitGesture.mockImplementationOnce(() => {
      throw new Error('host failure');
    });
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 150, 150, B1);
    // React reports an exception thrown from an event handler on window rather
    // than out of the dispatch.
    const cap = captureWindowErrors();
    try {
      pointerUp(h.svg, 150, 150);
    } finally {
      cap.stop();
    }
    expect(cap.errors.map(String)).toEqual(['Error: host failure']);

    h.setProps({ selection: new Set([11]) });
    h.callbacks.onCommitGesture.mockClear();
    // A gesture left live would make this press abort instead: the band around a
    // (where the failed commit left it) must select it.
    pointerDown(h.svg, 80, 80);
    pointerMove(h.svg, 120, 120, B1);
    pointerUp(h.svg, 120, 120);
    expect([...h.selection()]).toEqual([10]);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(auxX(h, 1)).toBe('400');
  });

  it('S-2: a refused flow create closes its name editor quietly, so a later tool change touches no selection', async () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)], selectedTool: 'flow', autoCommitEdits: false });
    h.clearMountCalls();
    pointerDown(h.svg, 300, 300);
    pointerMove(h.svg, 420, 300, B1);
    pointerUp(h.svg, 420, 300);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
    expect(h.query('[contenteditable]')).toBeNull();
    h.callbacks.onSetSelection.mockClear();
    h.setProps({ selectedTool: 'aux' });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
  });

  it('L-b/L8: clearing the selection mid label drag does not throw, and no label side sticks to the next selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100), makeAux(11, 'b', 400, 100)] });
    h.clearMountCalls();
    const text = h.queryAll('g.simlin-aux text')[0];
    pointerDown(text, 130, 100);
    pointerMove(text, 60, 100);
    h.setProps({ selection: new Set() });
    const cap = captureWindowErrors();
    try {
      pointerUp(text, 60, 100);
    } finally {
      cap.stop();
    }
    expect(cap.errors).toEqual([]);
    h.setProps({ selection: new Set([11]) });
    expect((h.queryAll('g.simlin-aux text')[1] as SVGElement).style.textAnchor).toBe('start');
  });

  it('L-c: a sub-threshold wobble previews no nudge', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)] });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 102, 102, B1);
    expect(auxX(h)).toBe('100');
    pointerUp(h.svg, 102, 102);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('P-5: a shift-press on a selected cloud deselects it and starts no drag', () => {
    const h = renderCanvas({ elements: [...stockCloud(), makeAux(10, 'a', 600, 600)], selection: new Set([2, 10]) });
    h.clearMountCalls();
    pointerDown(h.query('path.simlin-cloud')!, 300, 100, { shiftKey: true });
    expect([...h.selection()]).toEqual([10]);
    pointerMove(h.svg, 360, 160, { shiftKey: true, buttons: 1 });
    pointerUp(h.svg, 360, 160, { shiftKey: true });
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('presses are ignored while presses are disabled, and a gesture pressed before still ends cleanly', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'a', 100, 100)], pressesDisabled: true });
    h.clearMountCalls();
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 150, 150, B1);
    expect(auxX(h)).toBe('100');
    pointerUp(h.svg, 150, 150);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();

    h.setProps({ pressesDisabled: false });
    pointerDown(h.query('g.simlin-aux')!, 100, 100);
    pointerMove(h.svg, 150, 150, B1);
    h.setProps({ pressesDisabled: true });
    pointerUp(h.svg, 150, 150);
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
    expect(auxX(h)).toBe('150');
  });
});
