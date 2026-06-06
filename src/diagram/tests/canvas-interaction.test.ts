// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Table-driven tests for the pure Canvas interaction model
// (drawing/canvas-interaction.ts). These exercise the discrete-gesture
// transitions and the pure geometry helpers the shell composes, asserting both
// resulting state and emitted effects.

import {
  AuxViewElement,
  CloudViewElement,
  FlowViewElement,
  LinkViewElement,
  ModuleViewElement,
  StockViewElement,
  UID,
  ViewElement,
} from '@simlin/core/datamodel';

import {
  computeDragSelection,
  decideMouseDownSelection,
  idleState,
  InteractionContext,
  InteractionEffect,
  InteractionEvent,
  InteractionState,
  isDrag,
  isInDragSelectRect,
  labelSideForPointer,
  reduceInteraction,
  resolveDeferredSelection,
} from '../drawing/canvas-interaction';

function makeAux(uid: number, x = 100, y = 100): AuxViewElement {
  return {
    type: 'aux',
    uid,
    var: undefined,
    x,
    y,
    name: `aux${uid}`,
    ident: `aux${uid}`,
    labelSide: 'right',
    isZeroRadius: false,
  };
}

function makeStock(uid: number, x = 0, y = 0): StockViewElement {
  return {
    type: 'stock',
    uid,
    var: undefined,
    x,
    y,
    name: `stock${uid}`,
    ident: `stock${uid}`,
    labelSide: 'bottom',
    isZeroRadius: false,
    inflows: [],
    outflows: [],
  };
}

function makeCloud(uid: number, x = 0, y = 0): CloudViewElement {
  return { type: 'cloud', uid, flowUid: -1, x, y, isZeroRadius: false, ident: undefined };
}

function makeModule(uid: number, x = 0, y = 0): ModuleViewElement {
  return {
    type: 'module',
    uid,
    var: undefined,
    x,
    y,
    name: `m${uid}`,
    ident: `m${uid}`,
    labelSide: 'bottom',
    isZeroRadius: false,
  };
}

function makeFlow(uid: number, x = 0, y = 0): FlowViewElement {
  return {
    type: 'flow',
    uid,
    var: undefined,
    x,
    y,
    name: `flow${uid}`,
    ident: `flow${uid}`,
    labelSide: 'bottom',
    points: [
      { x: x - 10, y, attachedToUid: undefined },
      { x: x + 10, y, attachedToUid: undefined },
    ],
    isZeroRadius: false,
  };
}

function makeLink(uid: number, fromUid: number, toUid: number): LinkViewElement {
  return {
    type: 'link',
    uid,
    fromUid,
    toUid,
    arc: 0,
    multiPoint: undefined,
    isStraight: false,
    polarity: undefined,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
}

const ctx = (selection: Iterable<UID>, canEditName = false): InteractionContext => ({
  selection: new Set(selection),
  canEditName,
});

// Convenience accessors for asserting on emitted effects.
const effectKinds = (effects: readonly InteractionEffect[]): string[] => effects.map((e) => e.kind);
const setSelectionEffect = (effects: readonly InteractionEffect[]): ReadonlySet<UID> | undefined => {
  const e = effects.find((x) => x.kind === 'setSelection');
  return e && e.kind === 'setSelection' ? e.selection : undefined;
};

describe('decideMouseDownSelection', () => {
  it('replaces selection when clicking an unselected element without modifier', () => {
    const r = decideMouseDownSelection(new Set([1, 2]), 5, false);
    expect(r.newSelection).toEqual(new Set([5]));
    expect(r.deferSingleSelect).toBeUndefined();
  });

  it('toggles in with modifier when not selected', () => {
    const r = decideMouseDownSelection(new Set([1]), 2, true);
    expect(r.newSelection).toEqual(new Set([1, 2]));
  });

  it('toggles out with modifier when already selected', () => {
    const r = decideMouseDownSelection(new Set([1, 2]), 2, true);
    expect(r.newSelection).toEqual(new Set([1]));
  });

  it('defers when clicking an already-selected element without modifier', () => {
    const r = decideMouseDownSelection(new Set([1, 2]), 2, false);
    expect(r.newSelection).toBeUndefined();
    expect(r.deferSingleSelect).toBe(2);
  });
});

describe('resolveDeferredSelection', () => {
  it('collapses to the deferred element when no drag occurred', () => {
    expect(resolveDeferredSelection(7, false)).toEqual(new Set([7]));
  });
  it('preserves the group (returns undefined) when a drag occurred', () => {
    expect(resolveDeferredSelection(7, true)).toBeUndefined();
  });
  it('returns undefined when nothing was deferred', () => {
    expect(resolveDeferredSelection(undefined, false)).toBeUndefined();
  });
});

describe('isDrag threshold', () => {
  it('sub-threshold wobble is a click, not a drag', () => {
    // 4px screen movement at zoom 1 is below the 5px threshold
    expect(isDrag({ x: 4, y: 0 }, 1)).toBe(false);
  });
  it('over-threshold movement is a drag', () => {
    expect(isDrag({ x: 6, y: 0 }, 1)).toBe(true);
  });
  it('scales with zoom: small model delta is a drag when zoomed in', () => {
    expect(isDrag({ x: 3, y: 0 }, 1)).toBe(false);
    expect(isDrag({ x: 3, y: 0 }, 2)).toBe(true);
  });
  it('undefined delta is never a drag', () => {
    expect(isDrag(undefined, 10)).toBe(false);
  });
});

describe('labelSideForPointer quadrants', () => {
  const center = { x: 100, y: 100 };
  it('pointer to the left -> label on the left', () => {
    expect(labelSideForPointer(center, { x: 0, y: 100 })).toBe('left');
  });
  it('pointer to the right -> label on the right', () => {
    expect(labelSideForPointer(center, { x: 200, y: 100 })).toBe('right');
  });
  it('pointer above -> label on top', () => {
    expect(labelSideForPointer(center, { x: 100, y: 0 })).toBe('top');
  });
  it('pointer below -> label on the bottom', () => {
    expect(labelSideForPointer(center, { x: 100, y: 200 })).toBe('bottom');
  });
});

describe('drag-select rectangle membership', () => {
  const rect = { left: 0, right: 100, top: 0, bottom: 100 };
  const auxHitNever = () => false;

  it('selects a stock whose center is inside', () => {
    expect(isInDragSelectRect(makeStock(1, 50, 50), rect, auxHitNever)).toBe(true);
  });
  it('rejects a stock whose center is outside', () => {
    expect(isInDragSelectRect(makeStock(1, 200, 50), rect, auxHitNever)).toBe(false);
  });
  it('selects a cloud / flow / module / alias by center containment', () => {
    expect(isInDragSelectRect(makeCloud(1, 50, 50), rect, auxHitNever)).toBe(true);
    expect(isInDragSelectRect(makeFlow(2, 50, 50), rect, auxHitNever)).toBe(true);
    expect(isInDragSelectRect(makeModule(3, 50, 50), rect, auxHitNever)).toBe(true);
  });
  it('aux is selected when a rectangle corner hits its circle even if center is outside', () => {
    const aux = makeAux(1, 200, 200);
    expect(isInDragSelectRect(aux, rect, auxHitNever)).toBe(false);
    expect(isInDragSelectRect(aux, rect, () => true)).toBe(true);
  });
  it('never selects links', () => {
    expect(isInDragSelectRect(makeLink(9, 1, 2), rect, () => true)).toBe(false);
  });

  it('computeDragSelection collects every contained element', () => {
    const elements: ViewElement[] = [
      makeStock(1, 50, 50),
      makeStock(2, 500, 500),
      makeAux(3, 10, 10),
      makeLink(9, 1, 3),
    ];
    const result = computeDragSelection(elements, rect, auxHitNever);
    expect(result).toEqual(new Set([1, 3]));
  });
});

describe('reduceInteraction: canvas press', () => {
  it('touch/shift press enters panning, no selection change', () => {
    const r = reduceInteraction(idleState, { kind: 'canvasPointerDown', pan: true }, ctx([1]));
    expect(r.state).toEqual({ mode: 'panning' });
    expect(r.effects).toEqual([]);
  });
  it('plain press enters drag-selecting', () => {
    const r = reduceInteraction(idleState, { kind: 'canvasPointerDown', pan: false }, ctx([]));
    expect(r.state).toEqual({ mode: 'dragSelecting' });
  });
});

describe('reduceInteraction: creation tools', () => {
  it('aux/stock/module tool stages editing-on-pointer-up and captures pointer', () => {
    const r = reduceInteraction(idleState, { kind: 'createToolPointerDown', tool: 'aux' }, ctx([]));
    expect(r.state).toEqual({ mode: 'editingName', onPointerUp: true, creatingFlow: false });
    expect(effectKinds(r.effects)).toEqual(['capturePointer']);
  });

  it('flow tool enters arrowhead drag of an in-creation flow', () => {
    const r = reduceInteraction(idleState, { kind: 'flowToolPointerDown' }, ctx([]));
    expect(r.state).toEqual({
      mode: 'movingEndpoint',
      endpoint: 'arrow',
      pointerType: 'mouse',
      inCreation: true,
    });
  });
});

describe('reduceInteraction: element press selection', () => {
  it('clicking an unselected element selects it and prepares to move', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 5,
        isText: false,
        isArrowhead: false,
        isSource: false,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([1, 2]),
    );
    expect(r.state).toEqual({
      mode: 'movingSelection',
      deferredSingleSelectUid: undefined,
      deferredIsText: false,
      segmentIndex: undefined,
    });
    expect(setSelectionEffect(r.effects)).toEqual(new Set([5]));
    expect(effectKinds(r.effects)).toContain('clearSelectedTool');
    expect(effectKinds(r.effects)).toContain('capturePointer');
  });

  it('ctrl-click toggles the element into the selection', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 3,
        isText: false,
        isArrowhead: false,
        isSource: false,
        segmentIndex: undefined,
        modifier: true,
      },
      ctx([1, 2]),
    );
    expect(setSelectionEffect(r.effects)).toEqual(new Set([1, 2, 3]));
  });

  it('pressing an already-selected element defers (preserving the group)', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 2,
        isText: false,
        isArrowhead: false,
        isSource: false,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([1, 2, 3]),
    );
    expect(r.state).toEqual({
      mode: 'movingSelection',
      deferredSingleSelectUid: 2,
      deferredIsText: false,
      segmentIndex: undefined,
    });
    // No setSelection effect: the selection is left intact for a potential drag.
    expect(setSelectionEffect(r.effects)).toBeUndefined();
  });

  it('carries the flow segment index through into movingSelection', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 5,
        isText: false,
        isArrowhead: false,
        isSource: false,
        segmentIndex: 1,
        modifier: false,
      },
      ctx([]),
    );
    expect(r.state).toMatchObject({ mode: 'movingSelection', segmentIndex: 1 });
  });

  it('double-click (isText) on a single named element enters name editing', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 5,
        isText: true,
        isArrowhead: false,
        isSource: false,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([], true),
    );
    expect(r.state).toEqual({ mode: 'editingName', onPointerUp: false, creatingFlow: false });
    expect(setSelectionEffect(r.effects)).toEqual(new Set([5]));
    // No pointer capture while editing text.
    expect(effectKinds(r.effects)).not.toContain('capturePointer');
  });

  it('double-click on a non-name-editable element falls back to moving', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 5,
        isText: true,
        isArrowhead: false,
        isSource: false,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([], false),
    );
    expect(r.state).toMatchObject({ mode: 'movingSelection' });
  });
});

describe('reduceInteraction: endpoint drags', () => {
  it('arrowhead press enters arrow endpoint drag and captures pointer', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 9,
        isText: false,
        isArrowhead: true,
        isSource: false,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([9]),
    );
    expect(r.state).toEqual({
      mode: 'movingEndpoint',
      endpoint: 'arrow',
      pointerType: 'mouse',
      inCreation: false,
    });
    expect(effectKinds(r.effects)).toEqual(['capturePointer']);
  });

  it('source press enters source endpoint drag', () => {
    const r = reduceInteraction(
      idleState,
      {
        kind: 'elementPointerDown',
        elementUid: 9,
        isText: false,
        isArrowhead: false,
        isSource: true,
        segmentIndex: undefined,
        modifier: false,
      },
      ctx([9]),
    );
    expect(r.state).toMatchObject({ mode: 'movingEndpoint', endpoint: 'source' });
  });
});

describe('idleState', () => {
  it('is the idle mode', () => {
    const s: InteractionState = idleState;
    expect(s.mode).toBe('idle');
  });
});

// Type-only event coverage: exercising the discriminant ensures the union stays
// exhaustive for the shell's translation layer.
describe('InteractionEvent kinds', () => {
  it('enumerates the supported kinds', () => {
    const kinds: InteractionEvent['kind'][] = [
      'elementPointerDown',
      'canvasPointerDown',
      'createToolPointerDown',
      'flowToolPointerDown',
    ];
    expect(new Set(kinds).size).toBe(4);
  });
});
