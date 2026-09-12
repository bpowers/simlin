// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Reconciler-level gesture tests for element interactions of the React
// `Canvas`: flow pipe and valve drags, label drags, link and flow endpoint
// drags, creation tools, and name editing, driven with real pointer events. The
// harness applies each commit the way the controller publishes it, so a test
// sees what renders after the release. Assertions are on prop-callback payloads
// and rendered DOM only.
//
// What this establishes: the Canvas hit-tests presses into the planner,
// renders its plan while dragging, and commits exactly one GestureCommit (or
// nothing) on release. The geometry of each plan is gesture-planner*.test.ts;
// preview == commit frame by frame is canvas-gestures-preview-commit.test.tsx.

import { describe, it, expect, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import { fireEvent, act } from '@testing-library/react';

import type { FlowViewElement, LinkViewElement, ViewElement } from '@simlin/core/datamodel';

import type { GestureCommit } from '../drawing/Canvas';
import {
  makeAux,
  makeCloud,
  makeFlow,
  makeLink,
  makeStock,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
  type CanvasHarness,
} from './canvas-gesture-harness';
import { CloudRadius, StockWidth } from '../drawing/default';

function lastSelection(fn: Mock): number[] {
  const calls = fn.mock.calls;
  const last = calls[calls.length - 1];
  return last ? [...(last[0] as Set<number>)].sort((a, b) => a - b) : [];
}

function onlyCommit(h: CanvasHarness): GestureCommit {
  expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  return h.callbacks.onCommitGesture.mock.calls[0][0] as GestureCommit;
}

function committed<T extends ViewElement>(commit: GestureCommit, uid: number): T {
  const el = commit.elements.find((e) => e.uid === uid);
  expect(el).toBeDefined();
  return el as T;
}

// The inner flow path's polyline points from its `d` attribute. The final point
// is pulled back by the arrowhead inset, so use it for growth and orientation.
function flowPoints(h: CanvasHarness): Array<[number, number]> {
  const d = h.query('.simlin-flow .simlin-inner')?.getAttribute('d') ?? '';
  const nums = (d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
  const pts: Array<[number, number]> = [];
  for (let i = 0; i + 1 < nums.length; i += 2) {
    pts.push([nums[i], nums[i + 1]]);
  }
  return pts;
}

// Clouds render via transform="matrix(sx,0,0,sy, x-radius, y-radius)"; recover
// each cloud's center as (translateX + radius, translateY + radius).
function cloudCenters(h: CanvasHarness): Array<[number, number]> {
  return h.queryAll('.simlin-cloud').map((el) => {
    const t = el.getAttribute('transform') ?? '';
    const m = t.match(/matrix\([^,]+,[^,]+,[^,]+,[^,]+,\s*(-?\d+(?:\.\d+)?),\s*(-?\d+(?:\.\d+)?)\)/);
    return m ? [Number(m[1]) + CloudRadius, Number(m[2]) + CloudRadius] : [NaN, NaN];
  });
}

const hasCloudAt = (h: CanvasHarness, x: number, y: number): boolean =>
  cloudCenters(h).some(([cx, cy]) => Math.abs(cx - x) < 0.5 && Math.abs(cy - y) < 0.5);

// Stock (100,100) -> cloud (300,100), the source pinned to the stock's right face.
function stockToCloudFlow(): CanvasHarness {
  const stock = makeStock(1, 'stock', 100, 100);
  const cloud = makeCloud(2, 3, 300, 100);
  const flow = makeFlow(
    3,
    'flow',
    [
      { x: 122.5, y: 100, attachedToUid: 1 },
      { x: 300, y: 100, attachedToUid: 2 },
    ],
    { x: 200, y: 100 },
  );
  return renderCanvas({ elements: [stock, cloud, flow] });
}

describe('Canvas gestures: flow pipe and valve drags', () => {
  it('a perpendicular drag on a sole flow`s pipe offsets the pressed segment', () => {
    // stock(100,100) -> (300,100) -> (300,300) -> cloud(500,300); segment 1 is vertical.
    const stock = makeStock(1, 'stock', 100, 100);
    const cloud = makeCloud(2, 3, 500, 300);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 122.5, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: undefined },
        { x: 300, y: 300, attachedToUid: undefined },
        { x: 500, y: 300, attachedToUid: 2 },
      ],
      { x: 300, y: 200 },
    );
    const h = renderCanvas({ elements: [stock, cloud, flow] });
    h.clearMountCalls();

    pointerDown(h.query('.simlin-outer')!, 300, 260);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);
    pointerMove(h.svg, 340, 262, { buttons: 1 });
    pointerUp(h.svg, 340, 262);

    const commit = onlyCommit(h);
    expect(commit.label).toBe('pipe move');
    const f = committed<FlowViewElement>(commit, 3);
    expect(f.points.map((p) => [p.x, p.y])).toEqual([
      [122.5, 100],
      [340, 100],
      [340, 300],
      [500, 300],
    ]);
  });

  it('an along-axis drag on the valve slides it', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    pointerDown(h.query('.simlin-flow circle')!, 200, 100);
    pointerMove(h.svg, 240, 101, { buttons: 1 });
    pointerUp(h.svg, 240, 101);

    const f = committed<FlowViewElement>(onlyCommit(h), 3);
    expect([f.x, f.y]).toEqual([240, 100]);
    expect(f.points.map((p) => [p.x, p.y])).toEqual([
      [122.5, 100],
      [300, 100],
    ]);
  });
});

describe('Canvas gestures: label drag', () => {
  // The quadrant rule itself is tabled in gesture-planner-classify.test.ts; each
  // direction here is dragged from the label text node. The aux's label starts
  // on the right, so the right-hand drag commits nothing.
  it.each([
    ['left', 60, 100, 'left'],
    ['top', 100, 60, 'top'],
    ['bottom', 100, 140, 'bottom'],
  ] as const)('dragging the label toward the %s commits that side', (_name, toX, toY, side) => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, toX, toY);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);
    pointerUp(h.svg, toX, toY);

    const commit = onlyCommit(h);
    expect(commit.label).toBe('label move');
    expect(committed(commit, 10)).toMatchObject({ labelSide: side });
  });

  it('dragging the label to the side it already has commits nothing', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, 140, 100);
    pointerUp(h.svg, 140, 100);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('shows the label-side preview during the drag (text-anchor flips with the side)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    expect((h.query('.simlin-aux text') as SVGTextElement).style.textAnchor).toBe('start');

    pointerDown(text, 130, 100);
    pointerMove(text, 60, 100);
    expect((h.query('.simlin-aux text') as SVGTextElement).style.textAnchor).toBe('end');
  });
});

describe('Canvas gestures: link arrowhead drag', () => {
  function linkScene(): CanvasHarness {
    const from = makeAux(1, 'from', 100, 100);
    const to = makeAux(2, 'to', 300, 100);
    const other = makeAux(3, 'other', 300, 300);
    return renderCanvas({ elements: [from, to, other, makeLink(4, 1, 2)] });
  }

  it('releasing over a valid target commits the link ending there', () => {
    const h = linkScene();
    h.clearMountCalls();

    pointerDown(h.query('.simlin-arrowhead-link')!, 290, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([4]);
    pointerMove(h.svg, 300, 300, { buttons: 1 });
    pointerUp(h.svg, 300, 300);

    const l = committed<LinkViewElement>(onlyCommit(h), 4);
    expect(l.toUid).toBe(3);
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
  });

  it('releasing over empty space aborts: nothing commits and the link is kept', () => {
    const h = linkScene();
    h.clearMountCalls();

    pointerDown(h.query('.simlin-arrowhead-link')!, 290, 100);
    pointerMove(h.svg, 600, 600, { buttons: 1 });
    pointerUp(h.svg, 600, 600);

    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
    expect(h.query('.simlin-arrowhead-link')).not.toBeNull();
  });

  it('H1: a click on the arrowhead deletes nothing and commits nothing', () => {
    const h = linkScene();
    h.clearMountCalls();
    pointerDown(h.query('.simlin-arrowhead-link')!, 286, 100);
    pointerUp(h.svg, 286, 100);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
  });

  it('pressing a flow sink cloud selects the flow and drags its end', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();
    pointerDown(h.query('.simlin-cloud')!, 300, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);
  });
});

describe('Canvas gestures: flow endpoint drag', () => {
  it('dragging the arrowhead moves the sink cloud, keeping the grab offset', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    pointerDown(h.query('.simlin-arrowhead-flow')!, 290, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);
    pointerMove(h.svg, 340, 150, { buttons: 1 });
    pointerUp(h.svg, 340, 150);

    const commit = onlyCommit(h);
    const f = committed<FlowViewElement>(commit, 3);
    expect(f.points[f.points.length - 1]).toMatchObject({ x: 350, y: 150, attachedToUid: 2 });
    expect(committed(commit, 2)).toMatchObject({ x: 350, y: 150 });
  });

  it('dragging the source grip detaches it from the stock into a new cloud at the end', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    pointerDown(h.query('.simlin-flow rect[fill="transparent"]')!, 132, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);
    pointerMove(h.svg, 172, 200, { buttons: 1 });
    pointerUp(h.svg, 172, 200);

    const commit = onlyCommit(h);
    const f = committed<FlowViewElement>(commit, 3);
    const source = f.points[0];
    expect(source).toMatchObject({ x: 162.5, y: 200 });
    expect(committed(commit, source.attachedToUid!)).toMatchObject({ type: 'cloud', x: 162.5, y: 200, flowUid: 3 });
  });

  it('C0: a click on the source grip detaches nothing', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();
    pointerDown(h.query('.simlin-flow rect[fill="transparent"]')!, 132, 100);
    pointerUp(h.svg, 132, 100);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('the dragged end tracks the pointer before release, the cloud-to-cloud source staying put', () => {
    const source = makeCloud(1, 3, 100, 100);
    const sink = makeCloud(2, 3, 300, 100);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: 2 },
      ],
      { x: 200, y: 100 },
    );
    const h = renderCanvas({ elements: [source, sink, flow] });
    h.clearMountCalls();

    pointerDown(h.query('.simlin-arrowhead-flow')!, 290, 100);
    pointerMove(h.svg, 390, 100, { buttons: 1 });

    expect(hasCloudAt(h, 400, 100)).toBe(true);
    expect(hasCloudAt(h, 100, 100)).toBe(true);
    expect(flowPoints(h)[0]).toEqual([100, 100]);
    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
  });

  it('a sink cloud dragged over a stock snaps onto its face, and releasing there attaches the flow', () => {
    const stock = makeStock(1, 'stock', 100, 100);
    const cloud = makeCloud(2, 3, 300, 100);
    const target = makeStock(4, 'target', 400, 100);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 122.5, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: 2 },
      ],
      { x: 200, y: 100 },
    );
    const h = renderCanvas({ elements: [stock, cloud, target, flow] });
    h.clearMountCalls();

    pointerDown(h.query('.simlin-cloud')!, 300, 100);
    pointerMove(h.svg, 400, 100, { buttons: 1 });
    // The target stock renders as a valid target, and the dragged cloud is gone.
    expect(h.queryAll('.simlin-stock')[1].getAttribute('class')).toContain('targetGood');
    expect(h.query('.simlin-cloud')).toBeNull();
    pointerUp(h.svg, 400, 100);

    const f = committed<FlowViewElement>(onlyCommit(h), 3);
    expect(f.points[f.points.length - 1]).toMatchObject({ x: 400 - StockWidth / 2, y: 100, attachedToUid: 4 });
  });
});

describe('Canvas gestures: creation tools', () => {
  it.each([
    ['aux', 'aux', 'New Variable'],
    ['stock', 'stock', 'New Stock'],
    ['module', 'module', 'New Module'],
  ] as const)(
    '%s tool: the draft follows the drag, release opens name editing, Enter commits via onCreateVariable',
    (tool, type, expectedName) => {
      const h = renderCanvas({ elements: [], selectedTool: tool });
      h.clearMountCalls();

      pointerDown(h.svg, 200, 200);
      // A draft is not in the view: the press clears the selection and stages it.
      expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
      expect(h.query(`.simlin-${type}`)).not.toBeNull();

      pointerMove(h.svg, 230, 240, { buttons: 1 });
      // During the drag the draft still renders its own label and no editor is open.
      expect(h.query('[contenteditable]')).toBeNull();
      expect(h.query(`.simlin-${type} text`)?.textContent).toBe(expectedName);

      pointerUp(h.svg, 230, 240);
      const editable = h.query('[contenteditable]');
      expect(editable).not.toBeNull();

      act(() => {
        fireEvent.keyDown(editable!, { code: 'Enter' });
        fireEvent.keyUp(editable!, { code: 'Enter' });
      });
      expect(h.callbacks.onCreateVariable).toHaveBeenCalledTimes(1);
      const created = h.callbacks.onCreateVariable.mock.calls[0][0];
      expect(created).toMatchObject({ type, name: expectedName, x: 230, y: 240 });
      expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    },
  );

  it('flow tool: cancelling the just-created flow`s name edit deletes it', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 200, 200);
    pointerMove(h.svg, 300, 200, { buttons: 1 });
    pointerUp(h.svg, 300, 200);

    const commit = onlyCommit(h);
    expect(commit.label).toBe('flow creation');
    expect(commit.editName).toBe(commit.elements.find((e) => e.type === 'flow')!.uid);
    const editable = h.query('[contenteditable]');
    expect(editable).not.toBeNull();

    act(() => {
      fireEvent.keyUp(editable!, { code: 'Escape' });
    });
    expect(h.callbacks.onDeleteSelection).toHaveBeenCalledTimes(1);
  });

  it('flow tool: releasing does not crash when the host refuses the commit', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow', autoCommitEdits: false });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200);
    pointerMove(h.svg, 200, 200, { buttons: 1 });
    pointerMove(h.svg, 295, 200, { buttons: 1 });
    expect(() => pointerUp(h.svg, 300, 200)).not.toThrow();
    expect(h.callbacks.onCommitGesture).toHaveBeenCalledTimes(1);
  });

  it('flow tool: releasing the sink on a stock attaches the flow to that stock', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200);
    pointerMove(h.svg, 200, 200, { buttons: 1 });
    pointerMove(h.svg, 300, 200, { buttons: 1 });
    pointerUp(h.svg, 300, 200);

    const commit = onlyCommit(h);
    const f = commit.elements.find((e): e is FlowViewElement => e.type === 'flow')!;
    expect(f.points[f.points.length - 1]).toMatchObject({ x: 300 - StockWidth / 2, y: 200, attachedToUid: 1 });
  });
});

describe('Canvas gestures: flow tool live preview', () => {
  // As the user drags the flow tool, the drawn flow grows toward the pointer as
  // an orthogonal segment, its sink cloud at the pointer and its source cloud
  // planted at the press, and snaps onto a stock's face over a stock.
  it.each([
    ['right', { x: 100, y: 200 }, { x: 180, y: 200 }],
    ['down', { x: 200, y: 100 }, { x: 200, y: 220 }],
    ['left', { x: 300, y: 200 }, { x: 220, y: 200 }],
    ['up', { x: 200, y: 300 }, { x: 200, y: 220 }],
  ] as const)('grows %s toward the pointer with the sink cloud at the pointer', (_name, press, at) => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, press.x, press.y);
    pointerMove(h.svg, at.x, at.y, { buttons: 1 });

    const line = flowPoints(h);
    expect(line.length).toBe(2);
    expect(line[0][0] === line[1][0] || line[0][1] === line[1][1]).toBe(true);
    expect(hasCloudAt(h, press.x, press.y)).toBe(true);
    expect(hasCloudAt(h, at.x, at.y)).toBe(true);
  });

  it('a press within the click threshold previews nothing (E1)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();
    pointerDown(h.svg, 100, 200);
    pointerMove(h.svg, 102, 201, { buttons: 1 });
    expect(h.query('.simlin-flow')).toBeNull();
  });

  it('snaps onto the stock face when the pointer is over a stock', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200);
    pointerMove(h.svg, 295, 200, { buttons: 1 });

    const line = flowPoints(h);
    // The path ends at the face (277.5) less the arrowhead inset, not at the pointer.
    expect(line[line.length - 1][0]).toBeCloseTo(300 - StockWidth / 2 - 7.5);
    expect(h.queryAll('.simlin-cloud')).toHaveLength(1);
  });
});

describe('Canvas gestures: link and flow tools on an element', () => {
  it('link tool pressing a named element draws a link that commits onto the release target', () => {
    const h = renderCanvas({ elements: [makeAux(1, 'a', 100, 100), makeAux(2, 'b', 300, 100)], selectedTool: 'link' });
    h.clearMountCalls();

    pointerDown(h.queryAll('.simlin-aux')[0], 100, 100);
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    pointerMove(h.svg, 200, 130, { buttons: 1 });
    expect(h.query('.simlin-connector')).not.toBeNull();
    pointerMove(h.svg, 300, 100, { buttons: 1 });
    pointerUp(h.svg, 300, 100);

    const commit = onlyCommit(h);
    const l = commit.elements.find((e): e is LinkViewElement => e.type === 'link')!;
    expect({ fromUid: l.fromUid, toUid: l.toUid }).toEqual({ fromUid: 1, toUid: 2 });
    expect([...commit.selection]).toEqual([l.uid]);
  });

  it('flow tool pressing a stock draws a flow once the pointer moves', () => {
    const h = renderCanvas({ elements: [makeStock(1, 'stock', 100, 100)], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.query('.simlin-stock')!, 100, 100);
    expect(h.query('.simlin-flow')).toBeNull();
    pointerMove(h.svg, 220, 100, { buttons: 1 });
    expect(h.query('.simlin-flow')).not.toBeNull();
  });
});

describe('Canvas gestures: name editing', () => {
  function enterEditing(h: CanvasHarness): Element {
    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    return h.query('[contenteditable]')!;
  }

  it('double-clicking a named element`s label enters editing (EditableLabel overlay appears)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    expect(h.query('.editableLabel')).toBeNull();
    enterEditing(h);
    expect(h.query('.editableLabel')).not.toBeNull();
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);
  });

  it('Enter commits the rename via onRenameVariable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const editable = enterEditing(h);
    act(() => {
      fireEvent.keyDown(editable, { code: 'Enter' });
      fireEvent.keyUp(editable, { code: 'Enter' });
    });

    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
    expect(h.callbacks.onRenameVariable.mock.calls[0]).toEqual(['foo', 'foo']);
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
  });

  it('Escape cancels editing without a rename and clears the selection', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const editable = enterEditing(h);
    act(() => {
      fireEvent.keyUp(editable, { code: 'Escape' });
    });

    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('changing the selected tool ends editing (deferred commit)', async () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    enterEditing(h);
    expect(h.query('[contenteditable]')).not.toBeNull();

    h.setProps({ selectedTool: 'aux' });
    await act(async () => {
      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
  });

  it('a press on the overlay behind the editor commits the name', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    enterEditing(h);
    const overlay = h.query('[contenteditable]')!.closest('.overlay')!;
    pointerDown(overlay, 500, 500);
    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
  });
});

// Regression coverage for "double-clicking a var name doesn't reliably open the
// name editor": a double-click on an already-selected element's label must open
// the editor at once (a label double-click is its own press arm, never a
// deferred single select resolved on a pointer-up that already fired), and the
// label's own click threshold keeps a physical double-click's 1-2px wobble from
// starting a label drag.
describe('Canvas gestures: double-click name-edit reliability', () => {
  it('opens the editor when double-clicking the name of an ALREADY-SELECTED variable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    expect(h.query('[contenteditable]')).toBeNull();
    act(() => {
      fireEvent.doubleClick(h.query('.simlin-aux text')!, { clientX: 130, clientY: 100 });
    });
    expect(h.query('[contenteditable]')).not.toBeNull();
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
  });

  it('opens the editor when double-clicking the name of an UNSELECTED variable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();
    act(() => {
      fireEvent.doubleClick(h.query('.simlin-aux text')!, { clientX: 130, clientY: 100 });
    });
    expect(h.query('[contenteditable]')).not.toBeNull();
  });

  it('a sub-threshold wobble on the name label does not start a label drag', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, 132, 101);
    pointerUp(text, 132, 101);

    expect(h.callbacks.onCommitGesture).not.toHaveBeenCalled();
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
  });

  it('a supra-threshold drag on the name label still moves the label', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, 60, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);
    pointerUp(h.svg, 60, 100);
    expect(committed(onlyCommit(h), 10)).toMatchObject({ labelSide: 'left' });
  });

  it('captures the pointer on press (so an edge grip that leaves the label sub-threshold can still drag)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text') as SVGElement;
    const captureSpy = rs.spyOn(text, 'setPointerCapture');

    pointerDown(text, 130, 100, { pointerId: 7 });
    expect(captureSpy).toHaveBeenCalledWith(7);

    pointerMove(text, 132, 101, { pointerId: 7 });
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();

    pointerMove(text, 150, 130, { pointerId: 7 });
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);

    captureSpy.mockRestore();
  });
});

// A flow or link whose endpoint uid points at an element not present in the view
// is corrupt data -- transient (an undo rebuild, #817) or persisted (#812). The
// renderers skip the broken element rather than throw out of render.
describe('Canvas rendering: dangling element references (#812, #817)', () => {
  it('does not crash when a flow references a missing source/sink, and still renders healthy elements', () => {
    const goodAux = makeAux(10, 'healthy', 100, 100);
    const danglingFlow = makeFlow(
      3,
      'broken flow',
      [
        { x: 100, y: 100, attachedToUid: 998 },
        { x: 300, y: 100, attachedToUid: 999 },
      ],
      { x: 200, y: 100 },
    );

    let h!: CanvasHarness;
    expect(() => {
      h = renderCanvas({ elements: [goodAux, danglingFlow] });
    }).not.toThrow();

    expect(h.query('.simlin-flow')).toBeNull();
    expect(h.query('.simlin-aux')).not.toBeNull();
  });

  it('does not crash when a link references a missing from/to endpoint', () => {
    const goodAux = makeAux(10, 'healthy', 100, 100);
    const danglingLink = makeLink(20, 901, 902);

    let h!: CanvasHarness;
    expect(() => {
      h = renderCanvas({ elements: [goodAux, danglingLink] });
    }).not.toThrow();

    expect(h.query('.simlin-arrowhead-link')).toBeNull();
    expect(h.query('.simlin-aux')).not.toBeNull();
  });
});
