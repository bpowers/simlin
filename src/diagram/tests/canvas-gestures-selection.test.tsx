/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Reconciler-level gesture tests for selection behavior of the React `Canvas`
// (Piece 1a of the canvas-interaction migration; see
// docs/design-plans/2026-06-07-canvas-interaction-migration.md). These pin the
// CURRENT behavior of click-select, drag-select, modifier toggle,
// deferred-single-select collapse, group drag, and pointercancel reset, and are
// the gate for the subsequent class->tagged-union and class->hooks migrations.
// They assert only on prop-callback payloads and rendered DOM -- never on Canvas
// instance internals -- so they must survive Canvas becoming a function
// component unchanged.

import {
  makeAux,
  makeStock,
  pointerCancel,
  pointerDown,
  pointerMove,
  pointerUp,
  renderCanvas,
} from './canvas-gesture-harness';

const DRAG_RECT = 'rect.dragRectOverlay';

function lastSelection(fn: jest.Mock): number[] {
  const calls = fn.mock.calls;
  const last = calls[calls.length - 1];
  return last ? [...(last[0] as Set<number>)].sort((a, b) => a - b) : [];
}

describe('Canvas gestures: empty-canvas click (checklist 1)', () => {
  it('a pure click (no pointer move) clears the selection and shows no drag rect', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    pointerDown(h.svg, 500, 500);
    // No move: no drag rect renders (dragRect needs a dragSelectionPoint, only
    // set by a pointermove -- see Canvas.render's dragRect branch ~line 2439).
    expect(h.query(DRAG_RECT)).toBeNull();

    pointerUp(h.svg, 500, 500);
    // clearPointerState(true) -> onSetSelection(empty) (handlePointerCancel
    // final branch ~line 1243-1244).
    expect(h.callbacks.onSetSelection).toHaveBeenCalledTimes(1);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
  });

  // Pinned surprise: the empty-canvas press has NO sub-threshold guard. ANY
  // pointermove (even a 2px wobble) calls handleDragSelection, which sets
  // isDragSelecting+dragSelectionPoint unconditionally (Canvas.handleDragSelection
  // ~line 1674), so a drag rect renders and pointer-up routes through the
  // drag-select path -- unlike the element-press path, which DOES threshold (see
  // checklist 4). This is current behavior, not a harness artifact.
  it('a sub-threshold wobble on empty canvas still renders a drag rect (no threshold here)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    pointerDown(h.svg, 500, 500);
    pointerMove(h.svg, 502, 502, { buttons: 1 });
    expect(h.query(DRAG_RECT)).not.toBeNull();

    pointerUp(h.svg, 502, 502);
    // Empty rubber-band selects nothing, replacing the selection.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
  });
});

describe('Canvas gestures: drag-select (checklist 2)', () => {
  it('renders a drag rect during the drag and selects elements whose centers fall inside, replacing the selection', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 150, 150);
    const c = makeAux(3, 'c', 400, 400); // outside the rect, currently selected
    const h = renderCanvas({ elements: [a, b, c], selection: new Set([3]) });
    h.clearMountCalls();

    pointerDown(h.svg, 50, 50);
    pointerMove(h.svg, 200, 200, { buttons: 1 });
    expect(h.query(DRAG_RECT)).not.toBeNull();

    pointerUp(h.svg, 200, 200);
    // a and b centers are inside [50,50]-[200,200]; c is not; selection replaced.
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([1, 2]);
  });

  it('selects an aux whose center is outside the rect when its circle overlaps a rect corner (aux corner-hit rule)', () => {
    // Rect [50,50]-[100,100]. Aux center (105,105) is outside, but the aux
    // circle (AuxRadius=9) reaches the bottom-right corner (100,100):
    // dist = sqrt(50) ~= 7.07 < 9. See computeDragSelection's auxCornerHit.
    const reachable = makeAux(1, 'a', 105, 105);
    const farStock = makeStock(2, 's', 400, 400);
    const h = renderCanvas({ elements: [reachable, farStock] });
    h.clearMountCalls();

    pointerDown(h.svg, 50, 50);
    pointerMove(h.svg, 100, 100, { buttons: 1 });
    pointerUp(h.svg, 100, 100);

    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([1]);
  });

  it('does NOT select an aux whose circle is just out of corner reach', () => {
    // Aux center (115,115): dist to corner (100,100) = sqrt(450) ~= 21.2 > 9.
    const unreachable = makeAux(1, 'a', 115, 115);
    const h = renderCanvas({ elements: [unreachable] });
    h.clearMountCalls();

    pointerDown(h.svg, 50, 50);
    pointerMove(h.svg, 100, 100, { buttons: 1 });
    pointerUp(h.svg, 100, 100);

    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
  });
});

describe('Canvas gestures: element click (checklist 4)', () => {
  it('a clean click selects the element, opens the details panel, and does not move it for a sub-threshold wobble', () => {
    const a = makeAux(1, 'a', 100, 100);
    const h = renderCanvas({ elements: [a] });
    h.clearMountCalls();

    const node = h.query('.simlin-aux')!;
    pointerDown(node, 100, 100);
    // 2px wobble is under the 5px ClickDragThresholdPx -- not a drag.
    pointerMove(h.svg, 102, 102, { buttons: 1 });
    pointerUp(h.svg, 102, 102);

    // Immediate selection replace with the clicked uid.
    expect(h.callbacks.onSetSelection).toHaveBeenCalledWith(new Set([1]));
    // Sub-threshold wobble does not nudge the element (isDragMovement gate
    // ~line 1109) ...
    expect(h.callbacks.onMoveSelection).not.toHaveBeenCalled();
    // ... and opens the variable-details panel instead (shouldShowVariableDetails).
    expect(h.callbacks.onShowVariableDetails).toHaveBeenCalledTimes(1);
  });
});

describe('Canvas gestures: modifier-click toggle (checklist 5)', () => {
  it('meta-click adds an unselected element to the selection', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 200, 200);
    const h = renderCanvas({ elements: [a, b], selection: new Set([1]) });
    h.clearMountCalls();

    const nodes = h.queryAll('.simlin-aux');
    pointerDown(nodes[1], 200, 200, { metaKey: true });

    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([1, 2]);
  });

  it('meta-click removes an already-selected element from the selection', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 200, 200);
    const h = renderCanvas({ elements: [a, b], selection: new Set([1, 2]) });
    h.clearMountCalls();

    const nodes = h.queryAll('.simlin-aux');
    pointerDown(nodes[1], 200, 200, { ctrlKey: true });

    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([1]);
  });
});

describe('Canvas gestures: deferred single-select (checklist 6)', () => {
  it('pressing an already-selected element without a modifier defers selection, then collapses to it on a no-drag release', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 200, 200);
    const h = renderCanvas({ elements: [a, b], selection: new Set([1, 2]) });
    h.clearMountCalls();

    const nodes = h.queryAll('.simlin-aux');
    pointerDown(nodes[0], 100, 100);
    // Press defers: selection unchanged so a group drag could still proceed
    // (decideMouseDownSelection -> deferSingleSelect).
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();

    pointerUp(h.svg, 100, 100);
    // No drag -> collapse to the single pressed element (resolveDeferredSelection).
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([1]);
  });
});

describe('Canvas gestures: group drag (checklist 7)', () => {
  it('dragging an already-selected element past the threshold preserves the group and moves it', () => {
    const a = makeAux(1, 'a', 100, 100);
    const b = makeAux(2, 'b', 200, 200);
    const h = renderCanvas({ elements: [a, b], selection: new Set([1, 2]) });
    h.clearMountCalls();

    const nodes = h.queryAll('.simlin-aux');
    pointerDown(nodes[0], 100, 100);
    pointerMove(h.svg, 160, 160, { buttons: 1 });
    pointerUp(h.svg, 160, 160);

    // The deferred single-select is abandoned because a drag occurred, so the
    // group selection is preserved (no onSetSelection from this gesture).
    expect(h.callbacks.onSetSelection).not.toHaveBeenCalled();
    // The move is committed with the canvas-space delta (mouseDown - pointerUp).
    expect(h.callbacks.onMoveSelection).toHaveBeenCalledTimes(1);
    expect(h.callbacks.onMoveSelection.mock.calls[0][0]).toEqual({ x: -60, y: -60 });
  });
});

describe('Canvas gestures: pointercancel mid-gesture (checklist 16)', () => {
  it('cancelling a drag-select clears the rect and leaves a clean state for the next gesture', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)], selection: new Set([10]) });
    h.clearMountCalls();

    pointerDown(h.svg, 500, 500);
    pointerMove(h.svg, 520, 520, { buttons: 1 });
    expect(h.query(DRAG_RECT)).not.toBeNull();

    pointerCancel(h.svg, 520, 520);
    // The rubber-band is gone (drag state reset by clearPointerState).
    expect(h.query(DRAG_RECT)).toBeNull();

    // A subsequent fresh click on empty canvas behaves normally -- no stuck mode.
    h.callbacks.onSetSelection.mockClear();
    pointerDown(h.svg, 600, 600);
    pointerUp(h.svg, 600, 600);
    expect(lastSelection(h.callbacks.onSetSelection)).toEqual([]);
  });

  // Pinned surprise: pointercancel and pointerup are the SAME handler on the svg
  // (onPointerCancel and onPointerUp both -> handlePointerCancel, ~lines
  // 2515-2516), so a cancel mid element-drag COMMITS the move just like a
  // release rather than discarding it. Documenting current behavior; the
  // post-migration code must preserve it.
  it('cancelling an element drag commits the in-progress move (cancel == up today)', () => {
    const h = renderCanvas({ elements: [makeAux(10, 'foo', 100, 100)] });
    h.clearMountCalls();

    const node = h.query('.simlin-aux')!;
    pointerDown(node, 100, 100);
    pointerMove(h.svg, 160, 160, { buttons: 1 });
    pointerCancel(h.svg, 160, 160);

    expect(h.callbacks.onMoveSelection).toHaveBeenCalledTimes(1);
    expect(h.callbacks.onMoveSelection.mock.calls[0][0]).toEqual({ x: -60, y: -60 });
  });
});
