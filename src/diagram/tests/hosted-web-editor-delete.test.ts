/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// HostedWebEditor.handleDelete() issues DELETE /api/projects/:user/:name and,
// on success, navigates back to the project list. On any non-2xx/3xx response
// it throws so the in-editor confirmation dialog can surface the message and
// keep itself open for a retry. loadProject() (kicked off by the constructor)
// and redirectToHome() (a real navigation) are stubbed so the test can drive
// handleDelete() in isolation.

import { HostedWebEditor } from '../HostedWebEditor';

function mockFetch(impl: (input: string, init?: RequestInit) => Promise<Response>): jest.Mock {
  const fn = jest.fn(impl);
  (globalThis as unknown as { fetch: typeof fetch }).fetch = fn as unknown as typeof fetch;
  return fn;
}

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

function makeEditor(
  props: Partial<InstanceType<typeof HostedWebEditor>['props']> = {},
): InstanceType<typeof HostedWebEditor> {
  return new HostedWebEditor({
    username: 'alice',
    projectName: 'climate',
    readOnlyMode: false,
    ...props,
  } as InstanceType<typeof HostedWebEditor>['props']);
}

describe('HostedWebEditor.handleDelete', () => {
  const originalFetch = globalThis.fetch;
  let redirectSpy: jest.SpyInstance;

  beforeEach(() => {
    jest.spyOn(HostedWebEditor.prototype, 'loadProject').mockResolvedValue(undefined);
    redirectSpy = jest.spyOn(HostedWebEditor.prototype, 'redirectToHome').mockImplementation(() => {});
  });

  afterEach(() => {
    (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
    jest.restoreAllMocks();
  });

  test('DELETEs the project endpoint and navigates home on success', async () => {
    // `App.tsx` passes baseURL="" in production, so a relative path is the
    // realistic case.
    const fetchMock = mockFetch(async () => jsonResponse(200, {}));
    const editor = makeEditor({ username: 'alice', projectName: 'climate', baseURL: '' });

    await editor.handleDelete();

    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall).toBeDefined();
    expect(deleteCall![0]).toBe('/api/projects/alice/climate');
    expect((deleteCall![1] as RequestInit).credentials).toBe('same-origin');
    expect(redirectSpy).toHaveBeenCalledWith('/');
  });

  test('honors a custom baseURL for both the request and the redirect', async () => {
    const fetchMock = mockFetch(async () => jsonResponse(200, {}));
    const editor = makeEditor({ username: 'bob', projectName: 'world3', baseURL: 'https://example.test' });

    await editor.handleDelete();

    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall![0]).toBe('https://example.test/api/projects/bob/world3');
    expect(redirectSpy).toHaveBeenCalledWith('https://example.test/');
  });

  test('throws the server error message and does not navigate on failure', async () => {
    mockFetch(async () => jsonResponse(401, { error: 'unauthorized' }));
    const editor = makeEditor();

    await expect(editor.handleDelete()).rejects.toThrow('unauthorized');
    expect(redirectSpy).not.toHaveBeenCalled();
  });

  test('throws a status-bearing message when the error response has no body message', async () => {
    mockFetch(async () => jsonResponse(500, {}));
    const editor = makeEditor();

    await expect(editor.handleDelete()).rejects.toThrow(/500/);
    expect(redirectSpy).not.toHaveBeenCalled();
  });

  test('is a no-op in read-only mode', async () => {
    const fetchMock = mockFetch(async () => jsonResponse(200, {}));
    const editor = makeEditor({ readOnlyMode: true });

    await editor.handleDelete();

    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall).toBeUndefined();
    expect(redirectSpy).not.toHaveBeenCalled();
  });
});
