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
// and keep itself open for a retry. The function is framework-free, so these
// tests drive it directly with an injected fetch -- the HostedWebEditor shell's
// only added behavior is calling window.location.assign with the returned URL.

import { deleteProject, ProjectEndpoint } from '../hosted-web-editor-core';

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

function makeEndpoint(
  fetchImpl: (input: string, init?: RequestInit) => Promise<Response>,
  overrides: Partial<ProjectEndpoint> = {},
): ProjectEndpoint {
  return {
    base: '',
    username: 'alice',
    projectName: 'climate',
    fetch: fetchImpl as unknown as typeof fetch,
    ...overrides,
  };
}

describe('deleteProject', () => {
  test('DELETEs the project endpoint and returns the home URL on success', async () => {
    // `App.tsx` passes baseURL="" in production, so a relative path is the
    // realistic case.
    const fetchMock = jest.fn(async () => jsonResponse(200, {}));
    const endpoint = makeEndpoint(fetchMock, { base: '', username: 'alice', projectName: 'climate' });

    const homeUrl = await deleteProject(endpoint);

    expect(homeUrl).toBe('/');
    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall).toBeDefined();
    expect(deleteCall![0]).toBe('/api/projects/alice/climate');
    expect((deleteCall![1] as RequestInit).credentials).toBe('same-origin');
  });

  test('honors a custom base for both the request and the returned URL', async () => {
    const fetchMock = jest.fn(async () => jsonResponse(200, {}));
    const endpoint = makeEndpoint(fetchMock, {
      base: 'https://example.test',
      username: 'bob',
      projectName: 'world3',
    });

    const homeUrl = await deleteProject(endpoint);

    expect(homeUrl).toBe('https://example.test/');
    const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
    expect(deleteCall![0]).toBe('https://example.test/api/projects/bob/world3');
  });

  test('throws the server error message on failure', async () => {
    const endpoint = makeEndpoint(async () => jsonResponse(401, { error: 'unauthorized' }));

    await expect(deleteProject(endpoint)).rejects.toThrow('unauthorized');
  });

  test('throws a status-bearing message when the error response has no body message', async () => {
    const endpoint = makeEndpoint(async () => jsonResponse(500, {}));

    await expect(deleteProject(endpoint)).rejects.toThrow(/500/);
  });
});
