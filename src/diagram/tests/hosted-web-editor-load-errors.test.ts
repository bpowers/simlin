/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for HostedWebEditor.loadProject() error handling. The
// deferred componentDidMount call is fire-and-forget, so loadProject must
// never reject: a network error, a non-JSON body, or a response missing
// pb/version used to escape as an unhandled rejection and leave the editor
// permanently blank (render() returns an empty <div/> until projectBinary is
// set, and serviceErrors stayed empty so no message ever appeared).

import { fromUint8Array } from 'js-base64';

import { HostedWebEditor } from '../HostedWebEditor';

type HostedWebEditorInstance = InstanceType<typeof HostedWebEditor>;

function makeEditor(): HostedWebEditorInstance {
  const editor = new HostedWebEditor({
    username: 'alice',
    projectName: 'climate',
    baseURL: 'http://test.invalid',
    readOnlyMode: false,
  } as HostedWebEditorInstance['props']);

  Object.defineProperty(editor, 'state', {
    value: { ...editor.state },
    writable: true,
    configurable: true,
  });
  editor.setState = ((updater: unknown) => {
    const next = typeof updater === 'function' ? (updater as (s: unknown) => unknown)(editor.state) : updater;
    Object.assign(editor.state as object, next);
  }) as HostedWebEditorInstance['setState'];

  return editor;
}

function mockFetch(impl: () => Promise<unknown>): jest.Mock {
  const mock = jest.fn(impl);
  (globalThis as { fetch?: unknown }).fetch = mock;
  return mock;
}

describe('HostedWebEditor.loadProject() error handling', () => {
  afterEach(() => {
    delete (globalThis as { fetch?: unknown }).fetch;
  });

  it('surfaces a network-level fetch rejection as a service error', async () => {
    mockFetch(() => Promise.reject(new Error('connection refused')));
    const editor = makeEditor();

    await expect(editor.loadProject()).resolves.toBeUndefined();

    expect(editor.state.serviceErrors.length).toBeGreaterThan(0);
    expect(editor.state.serviceErrors[0].message).toContain('unable to load');
  });

  it('surfaces a non-JSON response body as a service error', async () => {
    mockFetch(async () => ({
      status: 200,
      json: () => Promise.reject(new SyntaxError('Unexpected token < in JSON')),
    }));
    const editor = makeEditor();

    await expect(editor.loadProject()).resolves.toBeUndefined();

    expect(editor.state.serviceErrors.length).toBeGreaterThan(0);
  });

  it('surfaces a response missing pb/version as a service error', async () => {
    mockFetch(async () => ({
      status: 200,
      json: async () => ({}),
    }));
    const editor = makeEditor();

    await expect(editor.loadProject()).resolves.toBeUndefined();

    expect(editor.state.serviceErrors.length).toBeGreaterThan(0);
    expect(editor.state.projectBinary).toBeUndefined();
  });

  it('still loads a well-formed response', async () => {
    const pb = new Uint8Array([1, 2, 3]);
    mockFetch(async () => ({
      status: 200,
      json: async () => ({ pb: fromUint8Array(pb), version: 4 }),
    }));
    const editor = makeEditor();

    await editor.loadProject();

    expect(editor.state.serviceErrors).toEqual([]);
    expect(editor.state.projectBinary).toEqual(pb);
    expect(editor.state.projectVersion).toBe(4);
  });
});
