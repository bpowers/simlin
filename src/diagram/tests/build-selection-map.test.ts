// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { buildSelectionMap, inCreationUid } from '../drawing/Canvas';
import { AuxViewElement, ViewElement } from '@simlin/core/datamodel';
import { UID } from '@simlin/core/common';
import { CanvasProps } from '../drawing/Canvas';

const makeAux = (uid: number, name: string): AuxViewElement => ({
  type: 'aux',
  uid,
  var: undefined,
  x: 100,
  y: 100,
  name,
  ident: name.toLowerCase().replace(/ /g, '_'),
  labelSide: 'right',
  isZeroRadius: false,
});

describe('buildSelectionMap', () => {
  const aux1 = makeAux(1, 'a');
  const aux2 = makeAux(2, 'b');
  const elements: ReadonlyMap<UID, ViewElement> = new Map([
    [aux1.uid, aux1],
    [aux2.uid, aux2],
  ]);

  it('maps selected UIDs to their elements', () => {
    const props = { selection: new Set([1, 2]) } as CanvasProps;
    const result = buildSelectionMap(props, elements);
    expect(result.size).toBe(2);
    expect(result.get(1)).toBe(aux1);
    expect(result.get(2)).toBe(aux2);
  });

  it('uses inCreation element when selection contains inCreationUid', () => {
    const inCreation = makeAux(inCreationUid, 'New Variable');
    const props = { selection: new Set([inCreationUid]) } as CanvasProps;
    const result = buildSelectionMap(props, elements, inCreation);
    expect(result.size).toBe(1);
    expect(result.get(inCreationUid)).toBe(inCreation);
  });

  it('skips inCreationUid when inCreation is undefined (async race)', () => {
    // This reproduces the crash: selection contains inCreationUid (-2)
    // but inCreation has been cleared (undefined) because Canvas.setState
    // runs synchronously while Editor.handleFlowAttach is still awaiting.
    const props = { selection: new Set([inCreationUid]) } as CanvasProps;
    const result = buildSelectionMap(props, elements, undefined);
    expect(result.size).toBe(0);
  });

  it('skips inCreationUid but keeps other selected elements', () => {
    const props = { selection: new Set([1, inCreationUid]) } as CanvasProps;
    const result = buildSelectionMap(props, elements, undefined);
    expect(result.size).toBe(1);
    expect(result.get(1)).toBe(aux1);
  });

  it('skips a selected UID that is no longer present in elements (async race after delete)', () => {
    // Reproduces the white-screen crash: when a selected connector is deleted,
    // Editor updates the view (removing the connector) before clearing the
    // selection, so there is a render where props.selection still references
    // the now-missing element. buildSelectionMap must not throw on it.
    const props = { selection: new Set([1, 13, 2]) } as CanvasProps;
    const result = buildSelectionMap(props, elements, undefined);
    expect(result.size).toBe(2);
    expect(result.get(1)).toBe(aux1);
    expect(result.get(2)).toBe(aux2);
    expect(result.has(13)).toBe(false);
  });

  it('returns an empty map when every selected UID is missing from elements', () => {
    const props = { selection: new Set([13, 14]) } as CanvasProps;
    const result = buildSelectionMap(props, elements, undefined);
    expect(result.size).toBe(0);
  });
});
