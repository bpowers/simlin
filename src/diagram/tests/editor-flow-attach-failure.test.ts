/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Issue #820: a failed flow-attach patch must not silently discard the drawn
// flow. handleFlowAttach used to early-return when the model-level patch failed,
// leaving the drawn flow uncommitted -- so the flow the user just drew vanished
// (the only feedback a transient toast) AND the just-created-flow name edit was
// left selecting a flow that was not in the view (the getElementByUid crash).
//
// The fix commits the optimistic view regardless of patch success, matching the
// sibling handlers (handleCreateVariable / handleSelectionDelete): the drawn
// flow stays on the canvas (and stays a real, selectable element), while the
// engine error still surfaces via the toast.
//
// This drives the real Editor + a real ProjectController wired to a fake engine
// scripted to reject every applyPatch (the corrupt-project failure mode). Canvas
// is mocked to a null renderer that captures the props (notably onMoveFlow =
// handleFlowAttach) so the flow-attach can be invoked directly.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render, screen } from '@testing-library/react';

import type { FlowViewElement, StockFlowView, ViewElement } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import { inCreationCloudUid, fauxCloudTargetUid } from '../drawing/creation-sentinels';

import { makeFakeEngine, validProjectJson } from './fake-engine';

// Capture the props the Editor hands to Canvas so a test can invoke onMoveFlow
// (handleFlowAttach) directly and read back the committed view/selection.
let capturedCanvasProps: Record<string, unknown> | undefined;
jest.mock('../drawing/Canvas', () => ({
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

async function flushTimers(): Promise<void> {
  for (let i = 0; i < 5; i++) {
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

describe('Editor flow-attach patch failure (issue #820)', () => {
  afterEach(() => {
    jest.restoreAllMocks();
    capturedCanvasProps = undefined;
  });

  it('preserves the drawn flow (and selects a real element) when the attach patch fails', async () => {
    // A fake engine that rejects every applyPatch models the corrupt-project
    // failure mode the issue was observed against.
    const engine = makeFakeEngine({ applyPatchThrows: true, json: validProjectJson() });
    jest.spyOn(EngineProject, 'openJson').mockResolvedValue(engine as unknown as EngineProject);

    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    await flushTimers();

    const props = capturedCanvasProps;
    if (!props) {
      throw new Error('Editor never rendered Canvas');
    }
    const onMoveFlow = props.onMoveFlow as (
      flow: FlowViewElement,
      targetUid: number,
      delta: { x: number; y: number },
      fauxTargetCenter: { x: number; y: number } | undefined,
      inCreation: boolean,
      isSourceAttach?: boolean,
    ) => Promise<void>;

    await act(async () => {
      await onMoveFlow(inCreationFlow(), 0, { x: -100, y: 0 }, { x: 300, y: 200 }, true, false);
    });
    await flushTimers();

    // The committed view must contain the drawn flow -- it was NOT discarded.
    const committedView = capturedCanvasProps!.view as StockFlowView;
    const flows = committedView.elements.filter((e: ViewElement): e is FlowViewElement => e.type === 'flow');
    expect(flows).toHaveLength(1);
    const flow = flows[0];

    // The flow carries a real (committed, non-sentinel) uid.
    expect(flow.uid).toBeGreaterThan(0);

    // The selection references that real, in-view element -- no phantom that a
    // later name-edit commit would dereference and crash on.
    const selection = capturedCanvasProps!.selection as ReadonlySet<number>;
    expect(selection.has(flow.uid)).toBe(true);
    expect(committedView.elements.some((e: ViewElement) => e.uid === flow.uid)).toBe(true);

    // The failure is NOT swallowed: the controller's onError surfaces the engine
    // error as a toast (the non-silent feedback the preserve-the-flow UX relies
    // on). The fake engine rejects with message 'patch rejected'.
    expect(screen.getAllByText('patch rejected').length).toBeGreaterThan(0);
  });
});
