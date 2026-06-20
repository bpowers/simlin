/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// deleteProject() issues DELETE /api/projects/:user/:name and, on success,
// returns the home URL the caller should navigate to. On any non-2xx/3xx
// response it throws so the in-editor confirmation dialog can surface the message
// and keep itself open for a retry. The function is framework-free and calls the
// global `fetch` directly (native fetch throws "Illegal invocation" when called as
// a method of any object but the global), so these tests stub `globalThis.fetch`.
// The HostedWebEditor shell's only added behavior is calling window.location.assign
// with the returned URL.

import { deleteProject, ProjectEndpoint } from '../hosted-web-editor-core';

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

const originalFetch = globalThis.fetch;
function installFetch(impl: (input: string, init?: RequestInit) => Promise<Response>): jest.Mock {
  const mock = jest.fn(impl);
  (globalThis as unknown as { fetch: typeof fetch }).fetch = mock as unknown as typeof fetch;
  return mock;
}
afterEach(() => {
  (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
});

describe('deleteProject', () => {
  test('DELETEs the project endpoint and returns the home URL on success', async () => {
    // `App.tsx` passes baseURL="" in production, so a relative path is the
    // realistic case.
    const fetchMock = installFetch(async () => jsonResponse(200, {}));
    const endpoint: ProjectEndpoint = { base: '', username: 'alice', projectName: 'climate' };

    const homeUrl = await deleteProject(endpoint);

    expect(homeUrl).toBe('/');
    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall).toBeDefined();
    expect(deleteCall![0]).toBe('/api/projects/alice/climate');
    expect((deleteCall![1] as RequestInit).credentials).toBe('same-origin');
  });

  test('honors a custom base for both the request and the returned URL', async () => {
    const fetchMock = installFetch(async () => jsonResponse(200, {}));
    const endpoint: ProjectEndpoint = { base: 'https://example.test', username: 'bob', projectName: 'world3' };

    const homeUrl = await deleteProject(endpoint);

    expect(homeUrl).toBe('https://example.test/');
    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall![0]).toBe('https://example.test/api/projects/bob/world3');
  });

  test('throws the server error message on failure', async () => {
    installFetch(async () => jsonResponse(401, { error: 'unauthorized' }));
    const endpoint: ProjectEndpoint = { base: '', username: 'alice', projectName: 'climate' };

    await expect(deleteProject(endpoint)).rejects.toThrow('unauthorized');
  });

  test('throws a status-bearing message when the error response has no body message', async () => {
    installFetch(async () => jsonResponse(500, {}));
    const endpoint: ProjectEndpoint = { base: '', username: 'alice', projectName: 'climate' };

    await expect(deleteProject(endpoint)).rejects.toThrow(/500/);
  });
});
