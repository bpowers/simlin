/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Thin integration tests for the Editor <-> ProjectController wiring.
//
// The save queue, undo history, and open/orphan-disposal logic all moved into
// ProjectController and are tested directly in project-controller.test.ts.
// What stays the Editor's responsibility is building the controller config
// correctly: forwarding props.onSave with the format matching inputFormat, and
// routing the controller's onError callback into the Editor's modelErrors toast
// list (presentation state the controller never owns).
//
// The Editor is now a function component, so these assert against OBSERVABLE
// behavior: we render a real <Editor> (which constructs its ProjectController
// during the lazy state init), capture the config the Editor passed to the
// ProjectController constructor by spying on it, then exercise the config's
// save() directly and drive onError() and assert the resulting toast appears in
// the rendered DOM. openInitialProject/dispose/scheduleSimRun are stubbed so the
// test stays off WASM.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { act, render, screen, RenderResult } from '@testing-library/react';

import * as ProjectControllerModule from '../project-controller';
import { Editor, type EditorProps } from '../Editor';

type ControllerConfig = ConstructorParameters<typeof ProjectControllerModule.ProjectController>[0];

function makeProps(overrides: Partial<EditorProps> = {}): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: '{}',
    initialProjectVersion: 1,
    name: 'p',
    onSave: async () => 1,
    ...overrides,
  } as EditorProps;
}

// Render <Editor> and return the config it handed to the ProjectController
// constructor. Stubs the engine-opening / dispose / sim-run methods so the
// component mounts in jsdom without WASM.
function renderAndCaptureConfig(props: EditorProps): { config: ControllerConfig; result: RenderResult } {
  jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
  jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
  jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});

  let captured: ControllerConfig | undefined;
  const real = ProjectControllerModule.ProjectController;
  jest.spyOn(ProjectControllerModule, 'ProjectController').mockImplementation((config: ControllerConfig) => {
    captured = config;
    return new real(config);
  });

  let result!: RenderResult;
  act(() => {
    result = render(React.createElement(Editor, props));
  });

  if (!captured) {
    throw new Error('Editor did not construct a ProjectController');
  }
  return { config: captured, result };
}

describe('Editor controller config wiring', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('forwards onSave as JsonProjectData when inputFormat is json', async () => {
    const onSave = jest.fn(async () => 7);
    const { config } = renderAndCaptureConfig(makeProps({ onSave }));

    const version = await config.save({ format: 'json', data: '{"a":1}' }, 3);

    expect(version).toBe(7);
    expect(onSave).toHaveBeenCalledWith({ format: 'json', data: '{"a":1}' }, 3);
  });

  it('forwards onSave as ProtobufProjectData when inputFormat is protobuf', async () => {
    const onSave = jest.fn(async () => 9);
    const bytes = new Uint8Array([1, 2, 3]);
    const { config } = renderAndCaptureConfig(
      makeProps({
        inputFormat: 'protobuf',
        initialProjectBinary: new Uint8Array([0]),
        onSave,
      } as unknown as Partial<EditorProps>),
    );

    const version = await config.save({ format: 'protobuf', data: bytes }, 4);

    expect(version).toBe(9);
    expect(onSave).toHaveBeenCalledWith({ format: 'protobuf', data: bytes }, 4);
  });

  it('routes controller onError into the Editor modelErrors toast list', () => {
    const { config } = renderAndCaptureConfig(makeProps());

    // onError is the controller config callback the Editor wires to its toast
    // list. Driving it must surface a toast in the rendered DOM.
    act(() => {
      config.onError(new Error('boom'));
    });

    expect(screen.getByText('boom')).toBeTruthy();
  });

  it('appends the read-only toast exactly once, even under StrictMode', () => {
    // The class appended the read-only toast on each componentDidMount; the
    // function component appends it from the mount effect, guarded by a
    // per-instance latch so React 18 StrictMode's mount/unmount/mount (state
    // preserved across the cycle) does not double-append. Render under
    // StrictMode and assert a single toast.
    jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    jest.spyOn(ProjectControllerModule.ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});

    act(() => {
      render(
        React.createElement(React.StrictMode, null, React.createElement(Editor, makeProps({ readOnlyMode: true }))),
      );
    });

    const toastText = "This is a read-only version. Any changes you make won't be saved.";
    expect(screen.getAllByText(toastText)).toHaveLength(1);
  });
});
