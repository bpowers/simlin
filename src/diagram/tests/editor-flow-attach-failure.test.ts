// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A failed flow-attach patch ROLLS BACK the drawn flow: the model and the
// diagram never disagree. The drawn flow renders at once (optimistically), the
// patch is rejected, and the rendered view returns to the committed one, which
// has no flow; the failure is reported once. (Committing the view anyway, the
// earlier policy for issue #820, saved a diagram whose flow names no variable.)
//
// This drives the real Editor and a real ProjectController wired to a fake
// engine that holds each patch behind a gate (the worker round trip) and then
// rejects it. Canvas is mocked to a null renderer that captures its props, so
// onMoveFlow (handleFlowAttach) can be invoked directly. What the Canvas does
// with a selection naming the rolled-back flow is pinned in
// canvas-gestures-flow-attach-failure.test.tsx.

import { describe, it, expect, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, render, screen } from '@testing-library/react';

import type { FlowViewElement, StockFlowView, ViewElement } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import { inCreationCloudUid, fauxCloudTargetUid } from '../drawing/creation-sentinels';

import { makeFakeEngine, makeGate, validProjectJson } from './fake-engine';

let capturedCanvasProps: Record<string, unknown> | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (props: Record<string, unknown>) => {
    capturedCanvasProps = props;
    return null;
  },
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

function makeProps(overrides: Partial<EditorProps> = {}): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: validProjectJson(),
    initialProjectVersion: 1,
    name: 'test-project',
    onSave: async () => 1,
    ...overrides,
  } as EditorProps;
}

async function flushUntil(condition: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !condition(); i++) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
}

// An in-creation flow drawn out of empty space toward empty space: source
// staged on the in-creation source cloud, sink on the faux sink target. This is
// the element Canvas passes to onMoveFlow at pointer-up, with targetUid 0 (no
// snap target) and a faux target center for the released sink position.
function inCreationFlow(): FlowViewElement {
  return {
    type: 'flow',
    uid: -2, // inCreationUid
    var: undefined,
    name: 'New Flow',
    ident: 'new_flow',
    x: 200,
    y: 200,
    labelSide: 'bottom',
    isZeroRadius: false,
    points: [
      { x: 200, y: 200, attachedToUid: inCreationCloudUid },
      { x: 200, y: 200, attachedToUid: fauxCloudTargetUid },
    ],
  };
}

function renderedView(): StockFlowView {
  return capturedCanvasProps!.view as StockFlowView;
}

function flows(view: StockFlowView): FlowViewElement[] {
  return view.elements.filter((e: ViewElement): e is FlowViewElement => e.type === 'flow');
}

describe('Editor flow-attach patch failure', () => {
  afterEach(() => {
    rs.restoreAllMocks();
    capturedCanvasProps = undefined;
  });

  it('renders the drawn flow at once, then rolls it back and reports once when the patch fails', async () => {
    const gate = makeGate();
    const engine = makeFakeEngine({
      applyPatchThrows: true,
      applyPatchGate: () => gate.wait(),
      json: validProjectJson(),
    });
    rs.spyOn(EngineProject, 'openJson').mockResolvedValue(engine as unknown as EngineProject);
    rs.spyOn(console, 'error').mockImplementation(() => {});

    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    await flushUntil(() => capturedCanvasProps?.view !== undefined);
    const onMoveFlow = capturedCanvasProps!.onMoveFlow as (
      flow: FlowViewElement,
      targetUid: number,
      delta: { x: number; y: number },
      fauxTargetCenter: { x: number; y: number } | undefined,
      inCreation: boolean,
      isSourceAttach?: boolean,
    ) => void;

    act(() => {
      onMoveFlow(inCreationFlow(), 0, { x: -100, y: 0 }, { x: 300, y: 200 }, true, false);
    });
    // Optimistic: the flow renders before the patch lands.
    expect(flows(renderedView())).toHaveLength(1);
    const drawn = flows(renderedView())[0];
    expect((capturedCanvasProps!.selection as ReadonlySet<number>).has(drawn.uid)).toBe(true);

    gate.open();
    await flushUntil(() => flows(renderedView()).length === 0);

    // Rolled back: the rendered view is the committed one -- no flow, no clouds.
    expect(flows(renderedView())).toHaveLength(0);
    expect(renderedView().elements.filter((e) => e.type === 'cloud')).toHaveLength(0);
    expect(engine.appliedPatches).toHaveLength(0);
    // Reported once, naming no discarded edits (there were none).
    expect(screen.getAllByText('patch rejected')).toHaveLength(1);
  });
});
