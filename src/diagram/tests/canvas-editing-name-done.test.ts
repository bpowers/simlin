/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for the just-created-flow name-edit state machine in
// Canvas.handleEditingNameDone. Post tagged-union migration (#65) the former
// `flowStillBeingCreated` flag lives as `interaction.editingName.creatingFlow`:
// it is set when a flow-creation drag finishes and the canvas enters name
// editing; cancelling that initial name edit deletes the just-created flow. The
// flag must be cleared once name editing ends (commit OR cancel) -- a stale
// `true` would make a later Escape-cancel of an unrelated rename delete that
// variable. The third scenario below pins exactly that regression and is the
// reason this file pokes the instance directly rather than going through the
// gesture harness: it must cross from one editing session into a later,
// unrelated one and observe that the flag did not leak.

import { Canvas, CanvasProps } from '../drawing/Canvas';
import { AuxViewElement, FlowViewElement, UID, ViewElement } from '@simlin/core/datamodel';
import { idleState, type InteractionState } from '../drawing/canvas-interaction';
import { plainDeserialize } from '../drawing/common';

type CanvasInstance = InstanceType<typeof Canvas>;

function makeAux(uid: number, name: string): AuxViewElement {
  return {
    type: 'aux',
    uid,
    var: undefined,
    x: 100,
    y: 100,
    name,
    ident: name.toLowerCase().replace(/ /g, '_'),
    labelSide: 'right',
    isZeroRadius: false,
  };
}

function makeFlow(uid: number, name: string): FlowViewElement {
  return {
    type: 'flow',
    uid,
    var: undefined,
    x: 100,
    y: 100,
    name,
    ident: name.toLowerCase().replace(/ /g, '_'),
    labelSide: 'bottom',
    points: [
      { x: 50, y: 100, attachedToUid: undefined },
      { x: 150, y: 100, attachedToUid: undefined },
    ],
    isZeroRadius: false,
  };
}

interface CanvasHarness {
  canvas: CanvasInstance;
  onDeleteSelection: jest.Mock;
  onRenameVariable: jest.Mock;
  setSelection: (uids: UID[]) => void;
}

function makeCanvas(elements: ViewElement[]): CanvasHarness {
  const onDeleteSelection = jest.fn();
  const onRenameVariable = jest.fn();
  const props = {
    embedded: false,
    selection: new Set<UID>(),
    selectedTool: undefined,
    onDeleteSelection,
    onRenameVariable,
    onSetSelection: jest.fn(),
    onCreateVariable: jest.fn(),
    onSelection: jest.fn(),
  } as unknown as CanvasProps;

  const canvas = new Canvas(props);

  // Shim React state management so we can drive the instance without the
  // reconciler (same pattern as editor-selection-changed.test.ts).
  Object.defineProperty(canvas, 'state', {
    value: { ...canvas.state },
    writable: true,
    configurable: true,
  });
  canvas.setState = ((updater: unknown) => {
    const next = typeof updater === 'function' ? (updater as (s: unknown) => unknown)(canvas.state) : updater;
    Object.assign(canvas.state as object, next);
  }) as CanvasInstance['setState'];

  canvas.elements = new Map(elements.map((el) => [el.uid, el]));

  const setSelection = (uids: UID[]) => {
    Object.defineProperty(canvas, 'props', {
      value: { ...props, selection: new Set(uids) },
      writable: true,
      configurable: true,
    });
  };

  return { canvas, onDeleteSelection, onRenameVariable, setSelection };
}

// The just-created-flow editing state as the post-migration union variant.
function creatingFlowEditing(): InteractionState {
  return { mode: 'editingName', onPointerUp: false, creatingFlow: true };
}

// A plain (non-flow) inline name edit, e.g. a double-click rename.
function plainEditing(): InteractionState {
  return { mode: 'editingName', onPointerUp: false, creatingFlow: false };
}

function creatingFlow(interaction: InteractionState): boolean {
  return interaction.mode === 'editingName' && interaction.creatingFlow;
}

describe('Canvas.handleEditingNameDone creatingFlow state machine', () => {
  it('cancelling the initial name edit of a just-created flow deletes it', () => {
    const flow = makeFlow(7, 'New Flow');
    const { canvas, onDeleteSelection, setSelection } = makeCanvas([flow]);
    setSelection([flow.uid]);

    canvas.setState({
      interaction: creatingFlowEditing(),
      editingName: plainDeserialize('label', 'New Flow'),
    });

    canvas.handleEditingNameDone(true);

    expect(onDeleteSelection).toHaveBeenCalledTimes(1);
    // clearPointerState resets interaction to idle (so creatingFlow is false).
    expect(canvas.state.interaction).toEqual(idleState);
    expect(creatingFlow(canvas.state.interaction)).toBe(false);
  });

  it('committing the initial flow name clears the flag', () => {
    const flow = makeFlow(7, 'New Flow');
    const { canvas, onRenameVariable, setSelection } = makeCanvas([flow]);
    setSelection([flow.uid]);

    canvas.setState({
      interaction: creatingFlowEditing(),
      editingName: plainDeserialize('label', 'inflow rate'),
    });

    canvas.handleEditingNameDone(false);

    expect(onRenameVariable).toHaveBeenCalledTimes(1);
    expect(canvas.state.interaction).toEqual(idleState);
    expect(creatingFlow(canvas.state.interaction)).toBe(false);
  });

  it('cancelling a later rename of an unrelated variable does NOT delete it', () => {
    const flow = makeFlow(7, 'New Flow');
    const aux = makeAux(9, 'Existing Variable');
    const { canvas, onDeleteSelection, setSelection } = makeCanvas([flow, aux]);

    // 1. The user finishes creating a flow and commits its name.
    setSelection([flow.uid]);
    canvas.setState({
      interaction: creatingFlowEditing(),
      editingName: plainDeserialize('label', 'inflow rate'),
    });
    canvas.handleEditingNameDone(false);

    // 2. Later, the user double-clicks an existing variable to rename it,
    //    then presses Escape to cancel. That cancel must not delete it -- the
    //    creatingFlow flag must not have leaked from the prior session.
    setSelection([aux.uid]);
    canvas.setState({
      interaction: plainEditing(),
      editingName: plainDeserialize('label', 'Existing Variable'),
    });
    canvas.handleEditingNameDone(true);

    expect(onDeleteSelection).not.toHaveBeenCalled();
  });
});
