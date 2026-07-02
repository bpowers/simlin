/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level gesture tests for element-edge interactions of the React
// `Canvas` (Piece 1a; see
// docs/design-plans/2026-06-07-canvas-interaction-migration.md): flow-segment
// drag, label drag, link/flow endpoint drag, creation tools, and name editing.
// Assertions are on prop-callback payloads and rendered DOM only.

import { fireEvent, act } from '@testing-library/react';

import type { StockFlowView } from '@simlin/core/datamodel';

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

function lastSelection(fn: jest.Mock): number[] {
  const calls = fn.mock.calls;
  const last = calls[calls.length - 1];
  return last ? [...(last[0] as Set<number>)].sort((a, b) => a - b) : [];
}

// Parse the rendered flow path's polyline points from its `d` attribute
// (e.g. "M100,200L180,200" -> [[100,200],[180,200]]). The inner flow path is
// drawn straight from the flow element's points, so this reflects the live
// geometry the user sees. NOTE: the line's final point is pulled back by a fixed
// glyph inset (finalAdjust) to leave room for the arrowhead, so use this for
// growth/orientation -- not for the exact endpoint (use arrowheadPoint for that).
function flowPoints(h: CanvasHarness): Array<[number, number]> {
  const inner = h.query('.simlin-flow .simlin-inner');
  const d = inner?.getAttribute('d') ?? '';
  const nums = (d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
  const pts: Array<[number, number]> = [];
  for (let i = 0; i + 1 < nums.length; i += 2) {
    pts.push([nums[i], nums[i + 1]]);
  }
  return pts;
}

// The flow arrowhead is drawn at the TRUE sink endpoint -- where the arrow
// actually points -- via transform="rotate(angle, x, y)". Extract (x, y).
function arrowheadPoint(h: CanvasHarness): [number, number] {
  const head = h.query('.simlin-arrowhead-bg');
  const t = head?.getAttribute('transform') ?? '';
  const m = t.match(/rotate\([^,]+,\s*(-?\d+(?:\.\d+)?),\s*(-?\d+(?:\.\d+)?)\)/);
  return m ? [Number(m[1]), Number(m[2])] : [NaN, NaN];
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

// A horizontal flow stock -> cloud, with the stock wired to the flow.
function stockToCloudFlow(): CanvasHarness {
  const stock = makeStock(1, 'stock', 100, 100);
  const cloud = makeCloud(2, 3, 300, 100);
  const flow = makeFlow(
    3,
    'flow',
    [
      { x: 100, y: 100, attachedToUid: 1 },
      { x: 300, y: 100, attachedToUid: 2 },
    ],
    { x: 200, y: 100 },
  );
  const stockWithFlow = { ...stock, outflows: [3] };
  return renderCanvas({ elements: [stockWithFlow, cloud, flow] });
}

describe('Canvas gestures: flow segment drag (checklist 8)', () => {
  it('dragging an interior flow segment plumbs the segmentIndex through onMoveSelection', () => {
    // L-shaped flow: stock(100,100) -> (300,100) -> (300,300) -> cloud(500,300).
    // The interior vertical segment is index 1.
    const stock = makeStock(1, 'stock', 100, 100);
    const cloud = makeCloud(2, 3, 500, 300);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: undefined },
        { x: 300, y: 300, attachedToUid: undefined },
        { x: 500, y: 300, attachedToUid: 2 },
      ],
      { x: 300, y: 200 },
    );
    const stockWithFlow = { ...stock, outflows: [3] };
    const h = renderCanvas({ elements: [stockWithFlow, cloud, flow] });
    h.clearMountCalls();

    const outer = h.query('.simlin-outer')!;
    // Press the vertical interior segment at (300,260) -- away from the valve
    // (300,200) so findClickedSegment returns the segment, not the valve.
    pointerDown(outer, 300, 260);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);

    pointerMove(h.svg, 340, 260, { buttons: 1 });
    pointerUp(h.svg, 340, 260);

    expect(h.callbacks.onMoveSelection).toHaveBeenCalledTimes(1);
    const [delta, , segmentIndex] = h.callbacks.onMoveSelection.mock.calls[0];
    expect(delta).toEqual({ x: -40, y: 0 });
    expect(segmentIndex).toBe(1);
  });
});

describe('Canvas gestures: label drag (checklist 9)', () => {
  // labelSideForPointer maps the pointer position relative to the element center
  // to a quadrant. Each direction is dragged from the label text node.
  it.each([
    ['left', 60, 100, 'left'],
    ['right', 140, 100, 'right'],
    ['top', 100, 60, 'top'],
    ['bottom', 100, 140, 'bottom'],
  ] as const)('dragging the label toward the %s fires onMoveLabel with that side', (_name, toX, toY, side) => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, toX, toY);
    // The label-drag selects the element (handleLabelDrag), which the host
    // commits so the pointer-up's only(selection) resolves.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);

    pointerUp(h.svg, toX, toY);
    expect(h.callbacks.onMoveLabel).toHaveBeenCalledWith(10, side);
  });

  it('shows the label-side preview during the drag (text-anchor flips with the side)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    // Default labelSide is 'right' -> text-anchor 'start'.
    expect((h.query('.simlin-aux text') as SVGTextElement).style.textAnchor).toBe('start');

    pointerDown(text, 130, 100);
    pointerMove(text, 60, 100); // drag to the left
    // During the drag the selected element's labelSide is overridden to 'left'
    // (deriveRenderState applies state.labelSide to selectionUpdates), so the
    // rendered label re-anchors to 'end'.
    expect((h.query('.simlin-aux text') as SVGTextElement).style.textAnchor).toBe('end');
  });
});

describe('Canvas gestures: link arrowhead drag (checklist 10)', () => {
  it('releasing over a valid target fires onAttachLink with that target ident', () => {
    const from = makeAux(1, 'from', 100, 100);
    const to = makeAux(2, 'to', 300, 100);
    const other = makeAux(3, 'other', 300, 300);
    const link = makeLink(4, 1, 2);
    const h = renderCanvas({ elements: [from, to, other, link] });
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-link')!;
    pointerDown(arrowhead, 290, 100);
    // Pressing the arrowhead enters reattachment and selects the link.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([4]);

    pointerMove(h.svg, 300, 300, { buttons: 1 }); // over 'other'
    pointerUp(h.svg, 300, 300);

    expect(h.callbacks.onAttachLink).toHaveBeenCalledTimes(1);
    const [linkArg, target] = h.callbacks.onAttachLink.mock.calls[0];
    expect(linkArg.uid).toBe(4);
    expect(target).toBe('other');
    expect(h.callbacks.onDeleteSelection).not.toHaveBeenCalled();
  });

  it('releasing over empty space with no invalid target deletes the link', () => {
    const from = makeAux(1, 'from', 100, 100);
    const to = makeAux(2, 'to', 300, 100);
    const link = makeLink(4, 1, 2);
    const h = renderCanvas({ elements: [from, to, link] });
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-link')!;
    pointerDown(arrowhead, 290, 100);
    pointerMove(h.svg, 600, 600, { buttons: 1 });
    pointerUp(h.svg, 600, 600);

    expect(h.callbacks.onAttachLink).not.toHaveBeenCalled();
    expect(h.callbacks.onDeleteSelection).toHaveBeenCalledTimes(1);
  });

  it('pressing a flow sink cloud swaps the selection to the flow (reattachment override)', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    const cloudNode = h.query('.simlin-cloud')!;
    pointerDown(cloudNode, 300, 100);
    // resolveSelectionForReattachment replaces the cloud uid with the flow uid.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);
  });
});

describe('Canvas gestures: flow endpoint drag (checklist 11)', () => {
  it('dragging the flow arrowhead (sink) fires onMoveFlow with isSourceAttach false', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-flow')!;
    pointerDown(arrowhead, 300, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);

    pointerMove(h.svg, 350, 150, { buttons: 1 });
    pointerUp(h.svg, 350, 150);

    expect(h.callbacks.onMoveFlow).toHaveBeenCalledTimes(1);
    const [flowArg, targetUid, delta, , inCreation, isSourceAttach] = h.callbacks.onMoveFlow.mock.calls[0];
    expect(flowArg.uid).toBe(3);
    expect(targetUid).toBe(0); // no valid stock under the cursor
    expect(delta).toEqual({ x: -50, y: -50 });
    expect(inCreation).toBe(false);
    expect(isSourceAttach).toBe(false);
  });

  it('dragging the flow source fires onMoveFlow with isSourceAttach true and a faux-target center', () => {
    const h = stockToCloudFlow();
    h.clearMountCalls();

    const sourceHit = h.query('.simlin-flow rect[fill="transparent"]')!;
    pointerDown(sourceHit, 110, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);

    pointerMove(h.svg, 150, 200, { buttons: 1 });
    pointerUp(h.svg, 150, 200);

    expect(h.callbacks.onMoveFlow).toHaveBeenCalledTimes(1);
    const [flowArg, targetUid, delta, fauxTargetCenter, , isSourceAttach] = h.callbacks.onMoveFlow.mock.calls[0];
    expect(flowArg.uid).toBe(3);
    expect(targetUid).toBe(0);
    expect(delta).toEqual({ x: -40, y: -100 });
    // Unattached source -> faux-target center = selectionCenterOffset - offset.
    expect(fauxTargetCenter).toEqual({ x: 110, y: 100 });
    expect(isSourceAttach).toBe(true);
  });
});

describe('Canvas gestures: existing cloud endpoint drag live preview', () => {
  // Regression: dragging an existing flow's cloud endpoint used to move only the
  // valve (UpdateFlow's valve-slide fallback), leaving the flow line/arrowhead
  // stale until pointer-up. It must now track the cursor DURING the drag, the
  // same as flow creation (both route through growEndpointDrag ->
  // UpdateCloudAndFlow). Assertions are made BEFORE pointer-up.

  it('sink cloud, along-axis drag: the flow line + arrowhead track the cursor mid-drag', () => {
    const h = stockToCloudFlow(); // stock(100,100) -> cloud(300,100), valve(200,100)
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-flow')!;
    pointerDown(arrowhead, 300, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);

    // Drag 100px further right along the axis; do NOT release.
    pointerMove(h.svg, 400, 100, { buttons: 1 });

    const head = arrowheadPoint(h);
    // Pre-fix the arrowhead stayed at the original cloud (300); now it follows
    // the cursor to 400 (the arrowhead sits at the true endpoint).
    expect(head[0]).toBeGreaterThan(360);
    expect(head[0]).toBeCloseTo(400);
    expect(head[1]).toBeCloseTo(100); // stayed horizontal
    // The rendered flow line grew to the moved endpoint (pulled back by the
    // finalAdjust arrowhead inset to ~392.5), not stuck at 300.
    const pts = flowPoints(h);
    expect(pts[pts.length - 1][0]).toBeGreaterThan(360);
  });

  it('sink cloud, vertical flow, along-axis drag: the arrowhead tracks down mid-drag', () => {
    const stock = makeStock(1, 'stock', 100, 100);
    const cloud = makeCloud(2, 3, 100, 300);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 100, y: 300, attachedToUid: 2 },
      ],
      { x: 100, y: 200 },
    );
    const h = renderCanvas({ elements: [{ ...stock, outflows: [3] }, cloud, flow] });
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-flow')!;
    pointerDown(arrowhead, 100, 300);
    pointerMove(h.svg, 100, 400, { buttons: 1 });

    const head = arrowheadPoint(h);
    expect(head[1]).toBeGreaterThan(360); // tracked downward
    expect(head[1]).toBeCloseTo(400);
    expect(head[0]).toBeCloseTo(100); // stayed vertical
  });

  it('source cloud, along-axis drag: the flow source tracks the cursor mid-drag', () => {
    const cloud = makeCloud(2, 3, 100, 100); // source
    const stock = makeStock(1, 'stock', 300, 100); // sink
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 100, y: 100, attachedToUid: 2 },
        { x: 300, y: 100, attachedToUid: 1 },
      ],
      { x: 200, y: 100 },
    );
    const h = renderCanvas({ elements: [{ ...stock, inflows: [3] }, cloud, flow] });
    h.clearMountCalls();

    const sourceHit = h.query('.simlin-flow rect[fill="transparent"]')!;
    pointerDown(sourceHit, 110, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([3]);

    // Drag the source 60px to the left; do NOT release.
    pointerMove(h.svg, 50, 100, { buttons: 1 });

    const pts = flowPoints(h);
    // Pre-fix the source point stayed at x=100; now it follows the cursor to 40.
    expect(pts[0][0]).toBeLessThan(80);
    expect(pts[0][0]).toBeCloseTo(40);
    expect(pts[0][1]).toBeCloseTo(100); // stayed horizontal
  });

  it('sink cloud dragged over a stock: the flow line snaps to the stock EDGE mid-drag', () => {
    const stock = makeStock(1, 'stock', 100, 100);
    const cloud = makeCloud(2, 3, 300, 100);
    const target = makeStock(4, 'target', 400, 100);
    const flow = makeFlow(
      3,
      'flow',
      [
        { x: 100, y: 100, attachedToUid: 1 },
        { x: 300, y: 100, attachedToUid: 2 },
      ],
      { x: 200, y: 100 },
    );
    const h = renderCanvas({ elements: [{ ...stock, outflows: [3] }, cloud, target, flow] });
    h.clearMountCalls();

    const arrowhead = h.query('.simlin-arrowhead-flow')!;
    pointerDown(arrowhead, 300, 100);
    // Move onto the target stock's center; do NOT release.
    pointerMove(h.svg, 400, 100, { buttons: 1 });

    const head = arrowheadPoint(h);
    // Pinned to the target's LEFT edge (400 - StockWidth/2 = 377.5), pulled back
    // by the arrowhead inset -- not the stock center (400) and not stuck at 300.
    expect(head[0]).toBeGreaterThan(355);
    expect(head[0]).toBeLessThan(378);
    expect(head[1]).toBeCloseTo(100);
  });

  it('cloud-to-cloud flow, sink drag: the non-dragged source cloud stays put mid-drag', () => {
    // Both endpoints are clouds. applyGroupMovement's UpdateFlow cloud-to-cloud
    // path would translate BOTH clouds by the delta; the fix holds the source
    // fixed and restores its cloud, so it stays attached to the fixed endpoint.
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

    const arrowhead = h.query('.simlin-arrowhead-flow')!;
    pointerDown(arrowhead, 300, 100);
    pointerMove(h.svg, 350, 100, { buttons: 1 }); // delta = {x:-50}; do NOT release

    // The flow's source endpoint stays fixed at (100,100)...
    const pts = flowPoints(h);
    expect(pts[0][0]).toBeCloseTo(100);
    expect(pts[0][1]).toBeCloseTo(100);
    // ...and the source cloud stays there too (pre-fix it drifted to x=150).
    const centers = cloudCenters(h);
    expect(centers.some(([cx, cy]) => Math.abs(cx - 100) < 0.5 && Math.abs(cy - 100) < 0.5)).toBe(true);
    expect(centers.some(([cx]) => Math.abs(cx - 150) < 0.5)).toBe(false);
  });
});

describe('Canvas gestures: creation tools (checklist 12)', () => {
  it.each([
    ['aux', 'aux', 'New Variable'],
    ['stock', 'stock', 'New Stock'],
    ['module', 'module', 'New Module'],
  ] as const)(
    '%s tool: press stages the element, release opens name editing, Enter commits via onCreateVariable',
    (tool, type, expectedName) => {
      const h = renderCanvas({ elements: [], selectedTool: tool });
      h.clearMountCalls();

      pointerDown(h.svg, 200, 200);
      // The in-creation element is staged and selected (uid inCreationUid = -2).
      expect(lastSelection(h.callbacks.onSetSelection)).toEqual([-2]);
      expect(h.query(`.simlin-${type}`)).not.toBeNull();

      pointerMove(h.svg, 210, 210, { buttons: 1 });

      // DURING the creation drag (after move, before pointer-up) the name editor
      // must NOT be active: the union is `editingName {onPointerUp: true}` (the
      // "start editing once the drag ends" staging handoff), which is distinct
      // from the editor being visible NOW. Asserting the old two-field semantics:
      //  (a) no inline contenteditable overlay is mounted yet, and
      //  (b) the staged element still renders its own text label (it is only
      //      suppressed once the editor actually shows on pointer-up).
      expect(h.query('[contenteditable]')).toBeNull();
      const stagedLabel = h.query(`.simlin-${type} text`);
      expect(stagedLabel).not.toBeNull();
      expect(stagedLabel!.textContent).toBe(expectedName);

      pointerUp(h.svg, 210, 210);

      // The creation drag releases into name editing (EditableLabel overlay).
      const editable = h.query('[contenteditable]');
      expect(editable).not.toBeNull();

      act(() => {
        fireEvent.keyDown(editable!, { code: 'Enter' });
        fireEvent.keyUp(editable!, { code: 'Enter' });
      });
      expect(h.callbacks.onCreateVariable).toHaveBeenCalledTimes(1);
      const created = h.callbacks.onCreateVariable.mock.calls[0][0];
      expect(created.type).toBe(type);
      expect(created.name).toBe(expectedName);
    },
  );

  it('flow tool: cancelling the just-created flow name edit deletes it (flowStillBeingCreated)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    // Model the host: onMoveFlow materializes a concrete flow + clouds and
    // selects the real flow (uid 50) -- mirroring Editor.handleFlowAttach.
    h.callbacks.onMoveFlow.mockImplementation(() => {
      const source = makeCloud(51, 50, 200, 200);
      const sink = makeCloud(52, 50, 300, 200);
      const flow = makeFlow(
        50,
        'New Flow',
        [
          { x: 200, y: 200, attachedToUid: 51 },
          { x: 300, y: 200, attachedToUid: 52 },
        ],
        { x: 250, y: 200 },
      );
      const view: StockFlowView = {
        nextUid: 53,
        elements: [source, sink, flow],
        viewBox: { x: 0, y: 0, width: 1000, height: 1000 },
        zoom: 1,
        useLetteredPolarity: false,
      };
      h.setProps({ view, selection: new Set([50]) });
    });

    pointerDown(h.svg, 200, 200);
    pointerMove(h.svg, 300, 200, { buttons: 1 });
    pointerUp(h.svg, 300, 200);

    const editable = h.query('[contenteditable]');
    expect(editable).not.toBeNull();

    act(() => {
      fireEvent.keyUp(editable!, { code: 'Escape' });
    });
    // Cancelling the initial flow name deletes the just-created flow.
    expect(h.callbacks.onDeleteSelection).toHaveBeenCalledTimes(1);
  });

  it('flow tool: releasing does not crash before the host commits the new selection (async attach)', () => {
    // Regression: Editor.handleFlowAttach commits the new flow's selection
    // asynchronously (after the engine round-trip). The pointer-up that creates
    // the flow enters name-editing AND clears the in-creation element in the
    // same commit, but props.selection still holds inCreationUid (-2) until that
    // async commit lands. The name-editor render must not dereference the
    // now-cleared in-creation element. Unlike the test above, onMoveFlow here
    // deliberately does NOT commit a selection, modeling that gap -- which is
    // exactly what the real (async) host does for one render.
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200); // empty space -> source cloud materializes
    pointerMove(h.svg, 200, 200, { buttons: 1 });
    pointerMove(h.svg, 295, 200, { buttons: 1 }); // drag the sink toward the stock
    expect(() => pointerUp(h.svg, 300, 200)).not.toThrow();

    expect(h.callbacks.onMoveFlow).toHaveBeenCalledTimes(1);
  });

  it('flow tool: releasing the sink on a stock attaches the flow to that stock', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200); // empty space, aligned in y with the stock
    pointerMove(h.svg, 200, 200, { buttons: 1 });
    pointerMove(h.svg, 300, 200, { buttons: 1 }); // cursor over the stock center
    pointerUp(h.svg, 300, 200);

    expect(h.callbacks.onMoveFlow).toHaveBeenCalledTimes(1);
    const [, targetUid] = h.callbacks.onMoveFlow.mock.calls[0];
    expect(targetUid).toBe(1); // attached to the stock (uid 1), not 0 (empty space)
  });
});

describe('Canvas gestures: flow tool live preview (a)', () => {
  // As the user drags the flow tool, the in-creation flow must GROW toward the
  // cursor as an orthogonal segment (it previously rendered as a zero-length,
  // invisible path), and snap to the stock's edge when the cursor is over a
  // valid stock. We assert on the rendered flow path geometry, not just presence.

  it('the in-creation flow grows orthogonally toward the cursor (not degenerate)', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200); // press empty space, level with the stock, to its left
    pointerMove(h.svg, 180, 200, { buttons: 1 }); // drag right, NOT yet over the stock

    const line = flowPoints(h);
    expect(line.length).toBeGreaterThanOrEqual(2);
    const lineStart = line[0];
    const lineEnd = line[line.length - 1];
    // grew a visible length (was a zero-length, invisible path before the fix)
    expect(Math.hypot(lineEnd[0] - lineStart[0], lineEnd[1] - lineStart[1])).toBeGreaterThan(50);
    // horizontal (orthogonal): the line stays on the source's y
    expect(lineEnd[1]).toBeCloseTo(lineStart[1]);
    // the arrowhead (true endpoint) points right at the cursor
    const head = arrowheadPoint(h);
    expect(head[0]).toBeCloseTo(180);
    expect(head[1]).toBeCloseTo(200);
  });

  it('the in-creation flow snaps to the stock edge when the cursor is over a stock', () => {
    const stock = makeStock(1, 'pop', 300, 200);
    const h = renderCanvas({ elements: [stock], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200); // press level with the stock
    pointerMove(h.svg, 180, 200, { buttons: 1 });
    pointerMove(h.svg, 295, 200, { buttons: 1 }); // cursor now over the stock (center 300)

    const head = arrowheadPoint(h);
    // pinned to the stock's LEFT edge (300 - StockWidth/2 = 277.5), NOT the cursor
    // (295) and NOT the stock center (300, which drew the arrowhead behind it)
    expect(head[0]).toBeCloseTo(300 - StockWidth / 2);
    expect(head[0]).not.toBeCloseTo(300);
    expect(head[0]).toBeLessThan(295); // snapped back to the edge, left of the cursor
    expect(head[1]).toBeCloseTo(200); // still horizontal
  });

  it('a dominant vertical drag grows a vertical in-creation flow', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 200, 100); // press empty space
    pointerMove(h.svg, 200, 220, { buttons: 1 }); // drag straight down

    const line = flowPoints(h);
    const lineStart = line[0];
    const lineEnd = line[line.length - 1];
    expect(Math.abs(lineEnd[1] - lineStart[1])).toBeGreaterThan(50); // grew downward
    // the arrowhead points straight down at the cursor (x unchanged, vertical)
    const head = arrowheadPoint(h);
    expect(head[0]).toBeCloseTo(200);
    expect(head[1]).toBeCloseTo(220);
  });

  it('grows LEFTWARD toward the cursor (not degenerate)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 300, 200); // press empty space
    pointerMove(h.svg, 220, 200, { buttons: 1 }); // drag LEFT

    const line = flowPoints(h);
    expect(Math.abs(line[line.length - 1][0] - line[0][0])).toBeGreaterThan(50); // grew
    const head = arrowheadPoint(h);
    expect(head[0]).toBeCloseTo(220); // arrowhead at the cursor, to the left
    expect(head[1]).toBeCloseTo(200);
  });

  it('grows UPWARD toward the cursor (not degenerate)', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 200, 300); // press empty space
    pointerMove(h.svg, 200, 220, { buttons: 1 }); // drag UP

    const head = arrowheadPoint(h);
    expect(head[0]).toBeCloseTo(200); // vertical
    expect(head[1]).toBeCloseTo(220); // arrowhead at the cursor, above
  });

  it('plants the source cloud at the tail, not on the arrowhead at the cursor', () => {
    const h = renderCanvas({ elements: [], selectedTool: 'flow' });
    h.clearMountCalls();

    pointerDown(h.svg, 100, 200); // press empty space -> source cloud planted here
    pointerMove(h.svg, 180, 200, { buttons: 1 }); // drag right

    const centers = cloudCenters(h);
    // the source cloud stays at the tail (the press point)...
    expect(centers.some(([x, y]) => Math.abs(x - 100) < 1 && Math.abs(y - 200) < 1)).toBe(true);
    // ...and does NOT ride along to the cursor/arrowhead at x=180
    expect(centers.some(([x]) => Math.abs(x - 180) < 1)).toBe(false);
  });
});

describe('Canvas gestures: link/flow tool from a named element (checklist 13)', () => {
  it('link tool pressing a named element starts an inCreation link drag that attaches on release', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 300, 100);
    const h = renderCanvas({ elements: [a, b], selectedTool: 'link' });
    h.clearMountCalls();

    const nodes = h.queryAll('.simlin-aux');
    pointerDown(nodes[0], 100, 100);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([-2]); // inCreation link
    expect(h.query('.simlin-connector')).not.toBeNull();

    pointerMove(h.svg, 300, 100, { buttons: 1 }); // onto b
    pointerUp(h.svg, 300, 100);

    expect(h.callbacks.onAttachLink).toHaveBeenCalledTimes(1);
    const [linkArg, target] = h.callbacks.onAttachLink.mock.calls[0];
    expect(linkArg.fromUid).toBe(1);
    expect(target).toBe('b');
  });

  it('flow tool pressing a stock starts an inCreation flow drag', () => {
    const s = makeStock(1, 'stock', 100, 100);
    const h = renderCanvas({ elements: [s], selectedTool: 'flow' });
    h.clearMountCalls();

    const node = h.query('.simlin-stock')!;
    pointerDown(node, 100, 100);

    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([-2]); // inCreation flow
    // The in-creation flow renders (a second .simlin-flow exists beyond none).
    expect(h.query('.simlin-flow')).not.toBeNull();
  });
});

describe('Canvas gestures: name editing (checklist 15)', () => {
  function enterEditing(h: CanvasHarness): Element {
    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    // Host commits the selection the double-click requested.
    h.setProps({ selection: new Set([10]) });
    return h.query('[contenteditable]')!;
  }

  it('double-clicking a single named element enters editing (EditableLabel overlay appears)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    expect(h.query('.editableLabel')).toBeNull();
    enterEditing(h);
    expect(h.query('.editableLabel')).not.toBeNull();
  });

  it('Enter commits the rename via onRenameVariable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const editable = enterEditing(h);
    act(() => {
      // EditableLabel commits on Enter held with a modifier.
      fireEvent.keyDown(editable, { code: 'Enter' });
      fireEvent.keyUp(editable, { code: 'Enter' });
    });

    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
    // No text was typed, so the name round-trips unchanged.
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
    // clearPointerState() on cancel clears the selection.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
    expect(h.query('[contenteditable]')).toBeNull();
  });

  it('changing the selected tool ends editing (deferred commit)', async () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    enterEditing(h);
    expect(h.query('[contenteditable]')).not.toBeNull();

    // render() schedules handleEditingNameDone(false) when selectedTool changes.
    h.setProps({ selectedTool: 'aux' });
    await act(async () => {
      await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(h.callbacks.onRenameVariable).toHaveBeenCalledTimes(1);
  });
});

// Regression coverage for "double-clicking a var name doesn't reliably open the
// name editor". Two independent unreliability sources are pinned here:
//
//  1. An ALREADY-SELECTED element's name-edit request was routed through the
//     deferred-single-select dance (computeMouseDownSelection returns
//     deferSingleSelect when a modifier-less press lands on a selected element).
//     That path only opens the editor on the *pointer-up* that resolves the
//     defer -- but a double-click's terminal event is `dblclick`, whose
//     pointer-up already fired, so the editor never appeared. It worked when the
//     element was unselected (that path selects + edits synchronously), so the
//     bug surfaced as "sometimes works, sometimes doesn't".
//
//  2. The name label started a label-drag on ANY pointer movement (no
//     click-vs-drag threshold), so the incidental 1-2px wobble of a physical
//     double-click was treated as a drag -- selecting the element and moving its
//     label instead of editing.
describe('Canvas gestures: double-click name-edit reliability', () => {
  it('opens the editor when double-clicking the name of an ALREADY-SELECTED variable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    expect(h.query('[contenteditable]')).toBeNull();

    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    // Mirror the host committing the selection the double-click requested.
    h.setProps({ selection: new Set([10]) });

    expect(h.query('[contenteditable]')).not.toBeNull();
    expect(h.callbacks.onRenameVariable).not.toHaveBeenCalled();
  });

  it('opens the editor when double-clicking the name of an UNSELECTED variable', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    act(() => {
      fireEvent.doubleClick(text, { clientX: 130, clientY: 100 });
    });
    h.setProps({ selection: new Set([10]) });

    expect(h.query('[contenteditable]')).not.toBeNull();
  });

  it('a sub-threshold wobble on the name label does not start a label drag', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, 132, 101); // ~2px, below the 5px click/drag threshold
    pointerUp(text, 132, 101);

    // No drag => no label move and no drag-driven selection.
    expect(h.callbacks.onMoveLabel).not.toHaveBeenCalled();
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
  });

  it('a supra-threshold drag on the name label still moves the label', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const text = h.query('.simlin-aux text')!;
    pointerDown(text, 130, 100);
    pointerMove(text, 60, 100); // 70px to the left, well past the threshold
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([10]);

    pointerUp(h.svg, 60, 100);
    expect(h.callbacks.onMoveLabel).toHaveBeenCalledWith(10, 'left');
  });
});

// A flow or link whose endpoint UID points at an element that is not present in
// the view is a corrupt/dangling reference. It can arise transiently (an undo
// rebuild that renders before the project swaps, issue #817) or be persisted in
// model data (issue #812, which left the whole model permanently uneditable).
// The element renderers must degrade gracefully -- skip the broken element --
// rather than throw out of render and take the entire editor down via the
// ErrorBoundary.
describe('Canvas rendering: dangling element references (#812, #817)', () => {
  it('does not crash when a flow references a missing source/sink, and still renders healthy elements', () => {
    const goodAux = makeAux(10, 'healthy', 100, 100);
    // sink point attaches to uid 999, which does not exist in the view.
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

    // The broken flow is skipped; the healthy aux still renders.
    expect(h.query('.simlin-flow')).toBeNull();
    expect(h.query('.simlin-aux')).not.toBeNull();
  });

  it('does not crash when a link references a missing from/to endpoint', () => {
    const goodAux = makeAux(10, 'healthy', 100, 100);
    // from/to reference uids that do not exist.
    const danglingLink = makeLink(20, 901, 902);

    let h!: CanvasHarness;
    expect(() => {
      h = renderCanvas({ elements: [goodAux, danglingLink] });
    }).not.toThrow();

    expect(h.query('.simlin-arrowhead-link')).toBeNull();
    expect(h.query('.simlin-aux')).not.toBeNull();
  });
});
