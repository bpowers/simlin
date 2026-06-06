/**
 * @jest-environment node
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
// We capture the config the Editor passes to the ProjectController constructor
// by spying on ProjectController, then exercise the config's save/onError.

import * as ProjectControllerModule from '../project-controller';
import { Editor } from '../Editor';

type EditorInstance = InstanceType<typeof Editor>;
type ControllerConfig = ConstructorParameters<typeof ProjectControllerModule.ProjectController>[0];

function captureConfig(props: EditorInstance['props']): { config: ControllerConfig; editor: EditorInstance } {
  let captured: ControllerConfig | undefined;
  const real = ProjectControllerModule.ProjectController;
  const spy = jest
    .spyOn(ProjectControllerModule, 'ProjectController')
    .mockImplementation((config: ControllerConfig) => {
      captured = config;
      return new real(config);
    });
  try {
    const editor = new Editor(props);
    if (!captured) {
      throw new Error('Editor did not construct a ProjectController');
    }
    return { config: captured, editor };
  } finally {
    spy.mockRestore();
  }
}

describe('Editor controller config wiring', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('forwards onSave as JsonProjectData when inputFormat is json', async () => {
    const onSave = jest.fn(async () => 7);
    const { config } = captureConfig({
      inputFormat: 'json',
      initialProjectJson: '{}',
      initialProjectVersion: 1,
      name: 'p',
      onSave,
    } as unknown as EditorInstance['props']);

    const version = await config.save({ format: 'json', data: '{"a":1}' }, 3);

    expect(version).toBe(7);
    expect(onSave).toHaveBeenCalledWith({ format: 'json', data: '{"a":1}' }, 3);
  });

  it('forwards onSave as ProtobufProjectData when inputFormat is protobuf', async () => {
    const onSave = jest.fn(async () => 9);
    const bytes = new Uint8Array([1, 2, 3]);
    const { config } = captureConfig({
      inputFormat: 'protobuf',
      initialProjectBinary: new Uint8Array([0]),
      initialProjectVersion: 1,
      name: 'p',
      onSave,
    } as unknown as EditorInstance['props']);

    const version = await config.save({ format: 'protobuf', data: bytes }, 4);

    expect(version).toBe(9);
    expect(onSave).toHaveBeenCalledWith({ format: 'protobuf', data: bytes }, 4);
  });

  it('routes controller onError into the Editor modelErrors toast list', () => {
    const { config, editor } = captureConfig({
      inputFormat: 'json',
      initialProjectJson: '{}',
      initialProjectVersion: 1,
      name: 'p',
      onSave: async () => 1,
    } as unknown as EditorInstance['props']);

    // Shim setState so we can observe the appended toast without React.
    editor.setState = ((updater: unknown) => {
      const next = typeof updater === 'function' ? (updater as (s: unknown) => unknown)(editor.state) : updater;
      Object.defineProperty(editor, 'state', {
        value: { ...editor.state, ...(next as object) },
        writable: true,
        configurable: true,
      });
    }) as EditorInstance['setState'];

    config.onError(new Error('boom'));

    expect(editor.state.modelErrors).toHaveLength(1);
    expect(editor.state.modelErrors[0].message).toBe('boom');
  });
});
