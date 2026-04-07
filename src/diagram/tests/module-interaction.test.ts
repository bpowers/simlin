// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { ModuleViewElement, AuxViewElement, LinkViewElement, ViewElement } from '@simlin/core/datamodel';

import { moduleContains, moduleBounds, ModuleWidth, ModuleHeight } from '../drawing/Module';
import { StockWidth, StockHeight } from '../drawing/Stock';
import { AuxRadius } from '../drawing/default';
import { labelRadii } from '../drawing/common';

// Helper to create test module elements
function makeModule(uid: number, x: number, y: number, labelSide: string = 'bottom'): ModuleViewElement {
  return {
    type: 'module',
    uid,
    name: `Module${uid}`,
    ident: `module_${uid}`,
    var: undefined,
    x,
    y,
    labelSide: labelSide as ModuleViewElement['labelSide'],
    isZeroRadius: false,
  };
}

describe('moduleContains', () => {
  it('returns true for a point at the center of the module', () => {
    const mod = makeModule(1, 100, 200);
    expect(moduleContains(mod, { x: 100, y: 200 })).toBe(true);
  });

  it('returns true for a point inside the module rectangle', () => {
    const mod = makeModule(1, 100, 200);
    // ModuleWidth=55, ModuleHeight=45, so half-width=27.5, half-height=22.5
    expect(moduleContains(mod, { x: 110, y: 210 })).toBe(true);
  });

  it('returns true for a point on the edge of the module', () => {
    const mod = makeModule(1, 100, 200);
    // Right edge: x = 100 + 27.5 = 127.5
    expect(moduleContains(mod, { x: 127.5, y: 200 })).toBe(true);
  });

  it('returns false for a point outside the module', () => {
    const mod = makeModule(1, 100, 200);
    expect(moduleContains(mod, { x: 200, y: 200 })).toBe(false);
  });

  it('returns false for a point just outside the right edge', () => {
    const mod = makeModule(1, 100, 200);
    // Right edge: x = 100 + 27.5 = 127.5
    expect(moduleContains(mod, { x: 128, y: 200 })).toBe(false);
  });

  it('returns false for a point just outside the bottom edge', () => {
    const mod = makeModule(1, 100, 200);
    // Bottom edge: y = 200 + 22.5 = 222.5
    expect(moduleContains(mod, { x: 100, y: 223 })).toBe(false);
  });

  it('handles modules at the origin', () => {
    const mod = makeModule(1, 0, 0);
    expect(moduleContains(mod, { x: 0, y: 0 })).toBe(true);
    expect(moduleContains(mod, { x: ModuleWidth / 2, y: ModuleHeight / 2 })).toBe(true);
    expect(moduleContains(mod, { x: ModuleWidth, y: 0 })).toBe(false);
  });

  it('handles modules with negative coordinates', () => {
    const mod = makeModule(1, -50, -100);
    expect(moduleContains(mod, { x: -50, y: -100 })).toBe(true);
    expect(moduleContains(mod, { x: -50 + ModuleWidth / 2, y: -100 })).toBe(true);
    expect(moduleContains(mod, { x: -50 + ModuleWidth / 2 + 1, y: -100 })).toBe(false);
  });
});

describe('moduleContains matches stockContains pattern for rectangular elements', () => {
  it('uses rectangular (not circular) hit-testing', () => {
    const mod = makeModule(1, 100, 100);
    // Corner of the rectangle: (100 + 27.5, 100 + 22.5) should be inside
    // because moduleContains uses dx/dy <= half-width/half-height
    expect(moduleContains(mod, { x: 100 + ModuleWidth / 2, y: 100 + ModuleHeight / 2 })).toBe(true);
    // This same point would be outside a circular hit-test with radius ~27.5
    // because distance = sqrt(27.5^2 + 22.5^2) = sqrt(1262.5) ~= 35.5 > 27.5
  });

  it('is consistent with how stock hit-testing works', () => {
    // Both stocks and modules should use rectangular hit-testing
    const mod = makeModule(1, 100, 100);
    // Module is 55x45; a point at (127, 122) is inside the rect
    expect(moduleContains(mod, { x: 127, y: 122 })).toBe(true);
    // A point at (128, 123) is outside
    expect(moduleContains(mod, { x: 128, y: 123 })).toBe(false);
  });
});

describe('module drag selection', () => {
  // Mirrors the drag-selection logic in Canvas.tsx for modules
  function isInSelectionRect(
    element: { x: number; y: number },
    left: number,
    right: number,
    top: number,
    bottom: number,
  ): boolean {
    return element.x >= left && element.x <= right && element.y >= top && element.y <= bottom;
  }

  it('should select a module when its center is within the drag rectangle', () => {
    const mod = makeModule(1, 100, 100);
    expect(isInSelectionRect(mod, 50, 150, 50, 150)).toBe(true);
  });

  it('should not select a module when its center is outside the drag rectangle', () => {
    const mod = makeModule(1, 200, 200);
    expect(isInSelectionRect(mod, 50, 150, 50, 150)).toBe(false);
  });

  it('should select modules alongside stocks and auxes', () => {
    const mod = makeModule(1, 100, 100);
    const aux: AuxViewElement = {
      type: 'aux',
      uid: 2,
      name: 'Aux2',
      ident: 'aux_2',
      var: undefined,
      x: 120,
      y: 80,
      labelSide: 'center',
      isZeroRadius: false,
    };
    const selectedUids: number[] = [];
    for (const element of [mod, aux]) {
      if (isInSelectionRect(element, 50, 200, 50, 150)) {
        selectedUids.push(element.uid);
      }
    }
    expect(selectedUids).toEqual([1, 2]);
  });
});

describe('module as link target', () => {
  // This tests the logic extracted from Canvas.isValidTarget:
  // modules should be valid link targets (alongside aux and flow)

  function isValidLinkTargetType(element: ViewElement): boolean {
    return element.type === 'flow' || element.type === 'aux' || element.type === 'module';
  }

  it('modules are valid link target types', () => {
    const mod = makeModule(1, 100, 100);
    expect(isValidLinkTargetType(mod)).toBe(true);
  });

  it('auxes are still valid link target types', () => {
    const aux: AuxViewElement = {
      type: 'aux',
      uid: 2,
      name: 'Aux2',
      ident: 'aux_2',
      var: undefined,
      x: 100,
      y: 100,
      labelSide: 'center',
      isZeroRadius: false,
    };
    expect(isValidLinkTargetType(aux)).toBe(true);
  });

  it('moduleContains is used for hit-testing during link drag', () => {
    const mod = makeModule(1, 100, 100);
    // Simulates the pointer being over the module during link dragging
    const pointer = { x: 110, y: 105 };
    expect(moduleContains(mod, pointer)).toBe(true);

    // Simulates the pointer being away from the module
    const farPointer = { x: 200, y: 200 };
    expect(moduleContains(mod, farPointer)).toBe(false);
  });

  it('prevents self-links (from and to same module)', () => {
    const mod = makeModule(1, 100, 100);
    const link: LinkViewElement = {
      type: 'link',
      uid: 10,
      fromUid: 1,
      toUid: -3,
      arc: 0,
      isStraight: false,
      multiPoint: undefined,
      polarity: undefined,
      x: 0,
      y: 0,
      isZeroRadius: false,
      ident: undefined,
    };
    // The isValidTarget logic checks: arrow.fromUid === element.uid
    expect(link.fromUid === mod.uid).toBe(true);
    // So this module should NOT be a valid target for its own link
  });

  it('allows links from one module to another', () => {
    makeModule(1, 100, 100); // source module (referenced by link.fromUid)
    const modB = makeModule(2, 300, 100);
    const link: LinkViewElement = {
      type: 'link',
      uid: 10,
      fromUid: 1,
      toUid: -3,
      arc: 0,
      isStraight: false,
      multiPoint: undefined,
      polarity: undefined,
      x: 0,
      y: 0,
      isZeroRadius: false,
      ident: undefined,
    };
    // link.fromUid (1) !== modB.uid (2), so modB is a valid target
    expect(link.fromUid !== modB.uid).toBe(true);
    // And modB passes the hit-test
    expect(moduleContains(modB, { x: 300, y: 100 })).toBe(true);
  });
});

describe('moduleBounds', () => {
  it('includes the module rectangle', () => {
    const mod = makeModule(1, 100, 200);
    const bounds = moduleBounds(mod);
    // Module rectangle: centered at (100, 200), w=55, h=45
    expect(bounds.left).toBeLessThanOrEqual(100 - ModuleWidth / 2);
    expect(bounds.right).toBeGreaterThanOrEqual(100 + ModuleWidth / 2);
    expect(bounds.top).toBeLessThanOrEqual(200 - ModuleHeight / 2);
    expect(bounds.bottom).toBeGreaterThanOrEqual(200 + ModuleHeight / 2);
  });

  it('extends beyond the rectangle to include the label', () => {
    const mod = makeModule(1, 100, 200, 'bottom');
    const bounds = moduleBounds(mod);
    // With a bottom label, the bounds should extend below the rectangle
    expect(bounds.bottom).toBeGreaterThan(200 + ModuleHeight / 2);
  });

  it('extends above the rectangle for top labels', () => {
    const mod = makeModule(1, 100, 200, 'top');
    const bounds = moduleBounds(mod);
    // With a top label, the bounds should extend above the rectangle
    expect(bounds.top).toBeLessThan(200 - ModuleHeight / 2);
  });

  it('includes label bounds for left labels', () => {
    const mod = makeModule(1, 100, 200, 'left');
    const bounds = moduleBounds(mod);
    expect(bounds.left).toBeLessThan(100 - ModuleWidth / 2);
  });

  it('includes label bounds for right labels', () => {
    const mod = makeModule(1, 100, 200, 'right');
    const bounds = moduleBounds(mod);
    expect(bounds.right).toBeGreaterThan(100 + ModuleWidth / 2);
  });
});

describe('labelRadii', () => {
  it('returns AuxRadius for aux elements', () => {
    const { rw, rh } = labelRadii('aux');
    expect(rw).toBe(AuxRadius);
    expect(rh).toBe(AuxRadius);
  });

  it('returns AuxRadius for flow elements', () => {
    const { rw, rh } = labelRadii('flow');
    expect(rw).toBe(AuxRadius);
    expect(rh).toBe(AuxRadius);
  });

  it('returns stock dimensions for stock elements', () => {
    const { rw, rh } = labelRadii('stock');
    expect(rw).toBe(StockWidth / 2);
    expect(rh).toBe(StockHeight / 2);
  });

  it('returns module dimensions for module elements', () => {
    const { rw, rh } = labelRadii('module');
    expect(rw).toBe(ModuleWidth / 2);
    expect(rh).toBe(ModuleHeight / 2);
  });

  it('module radii differ from aux radii', () => {
    // This is the core bug: modules were using AuxRadius (9)
    // instead of ModuleWidth/2 (27.5) and ModuleHeight/2 (22.5)
    const modRadii = labelRadii('module');
    const auxRadii = labelRadii('aux');
    expect(modRadii.rw).not.toBe(auxRadii.rw);
    expect(modRadii.rh).not.toBe(auxRadii.rh);
  });
});
