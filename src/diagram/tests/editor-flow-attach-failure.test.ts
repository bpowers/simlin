// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A drawn flow renders from release until its patch settles, and a failed
// patch ROLLS BACK the drawn flow: the model and the diagram never disagree.
// The drawn flow renders at once (the rendered view rule), and when the patch is
// rejected the rendered view returns to the committed one, which has no flow;
// the failure is reported once. (Committing the view anyway saved a diagram
// whose flow names no variable.)
//
// This drives the real Editor and a real ProjectController wired to a fake
// engine that holds each patch behind a gate (the worker round trip). Canvas is
// mocked to a null renderer that captures its props; the commit handed to
// onCommitGesture is planned by the production planner from those props, as the
// Canvas does at release. What the Canvas does with a selection naming the
// rolled-back flow is pinned in canvas-gestures-flow-attach-failure.test.tsx.

import { describe, it, expect, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, render, screen } from '@testing-library/react';

import type { FlowViewElement, StockFlowView, ViewElement } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';

import type { CanvasProps } from '../drawing/Canvas';
import { planGesture } from '../gesture-planner';
import { makeFakeEngine, makeGate, validProjectJson } from './fake-engine';

let capturedCanvasProps: CanvasProps | undefined;
// Every view the Canvas was rendered with, in order, so a frame published and
// replaced inside one flush is still seen.
const renderedViews: StockFlowView[] = [];
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (props: CanvasProps) => {
    capturedCanvasProps = props;
    renderedViews.push(props.view);
    return null;
  },
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

// The flow tool dragged from empty space into empty space, released: the
// commit the Canvas hands the Editor at pointer-up.
function drawFlow(): void {
  const p = capturedCanvasProps!;
  const plan = planGesture({
    view: p.view,
    variables: p.model.variables,
    selection: p.selection,
    gesture: { kind: 'createFlow', from: 'empty' },
    press: { x: 200, y: 200 },
    current: { x: 300, y: 200 },
    zoom: 1,
    pointerType: 'mouse',
    readOnly: false,
    names: p.newVariableName!,
  });
  expect(plan.commit).toBe('edit');
  p.onCommitGesture({
    label: plan.label,
    elements: plan.elements,
    nextUid: plan.nextUid,
    selection: plan.selection,
    token: p.token,
    baseView: p.view,
    editName: plan.handoff?.editName,
  });
}

function renderedView(): StockFlowView {
  return capturedCanvasProps!.view;
}

function flows(view: StockFlowView): FlowViewElement[] {
  return view.elements.filter((e: ViewElement): e is FlowViewElement => e.type === 'flow');
}

async function mount(applyPatchThrows: boolean) {
  const gate = makeGate();
  const engine = makeFakeEngine({
    applyPatchThrows,
    applyPatchGate: () => gate.wait(),
    json: validProjectJson(),
  });
  rs.spyOn(EngineProject, 'openJson').mockResolvedValue(engine as unknown as EngineProject);
  rs.spyOn(console, 'error').mockImplementation(() => {});

  act(() => {
    render(React.createElement(Editor, makeProps()));
  });
  await flushUntil(() => capturedCanvasProps?.view !== undefined);
  return { gate, engine };
}

describe('Editor flow-creation patch lifecycle', () => {
  afterEach(() => {
    rs.restoreAllMocks();
    capturedCanvasProps = undefined;
    renderedViews.length = 0;
  });

  // This stops at the engine accepting the patch: the fake's serialization is
  // static, so its read-back cannot carry the flow. The landed state against a
  // real engine is editor-engine-races.test.ts's job.
  it('F1: the drawn flow renders in every frame from release until the engine accepts the patch', async () => {
    const { gate, engine } = await mount(false);
    const releasedAt = renderedViews.length;

    act(() => drawFlow());
    const drawn = flows(renderedView());
    expect(drawn).toHaveLength(1);
    expect(engine.appliedPatches).toHaveLength(0);

    gate.open();
    await flushUntil(() => engine.appliedPatches.length === 1);
    expect(engine.appliedPatches).toHaveLength(1);
    const frames = renderedViews.slice(releasedAt);
    expect(frames.length).toBeGreaterThan(0);
    for (const view of frames) {
      expect(flows(view).map((f) => f.uid)).toEqual([drawn[0].uid]);
    }
  });

  it('renders the drawn flow at once, then rolls it back and reports once when the patch fails', async () => {
    const { gate, engine } = await mount(true);

    act(() => drawFlow());
    // Rendered before the patch lands.
    expect(flows(renderedView())).toHaveLength(1);
    const drawn = flows(renderedView())[0];
    expect(capturedCanvasProps!.selection.has(drawn.uid)).toBe(true);

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
