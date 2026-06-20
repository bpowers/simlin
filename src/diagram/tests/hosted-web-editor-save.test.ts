/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// saveProject() POSTs the serialized project and returns either the new server
// version (success) or an error message (any non-2xx/3xx response). The
// HostedWebEditor shell maps a `saved` result onto projectVersion and an `error`
// result onto its service-error list. The function is framework-free and calls the
// global `fetch` directly (native fetch throws "Illegal invocation" when called as
// a method of any object but the global), so these tests stub `globalThis.fetch`.

import { saveProject, ProjectEndpoint } from '../hosted-web-editor-core';
import type { ProtobufProjectData } from '../Editor';

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

const endpoint: ProjectEndpoint = { base: '', username: 'alice', projectName: 'climate' };

const originalFetch = globalThis.fetch;
function installFetch(impl: (input: string, init?: RequestInit) => Promise<Response>): jest.Mock {
  const mock = jest.fn(impl);
  (globalThis as unknown as { fetch: typeof fetch }).fetch = mock as unknown as typeof fetch;
  return mock;
}
afterEach(() => {
  (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
});

function makeProject(): ProtobufProjectData {
  return { data: new Uint8Array([1, 2, 3]) } as unknown as ProtobufProjectData;
}

describe('saveProject', () => {
  test('POSTs the project and returns the new version on success', async () => {
    const fetchMock = installFetch(async () => jsonResponse(200, { version: 7 }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result).toEqual({ kind: 'saved', version: 7 });
    const postCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'POST');
    expect(postCall).toBeDefined();
    expect(postCall![0]).toBe('/api/projects/alice/climate');
    const body = JSON.parse((postCall![1] as RequestInit).body as string);
    expect(body.currVersion).toBe(6);
    expect(typeof body.projectPB).toBe('string');
  });

  test('returns the server error message on a non-2xx response', async () => {
    installFetch(async () => jsonResponse(409, { error: 'version conflict' }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result).toEqual({ kind: 'error', message: 'version conflict' });
  });

  test('returns a status-bearing message when the error response has no body message', async () => {
    installFetch(async () => jsonResponse(500, {}));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.message).toMatch(/500/);
    }
  });
});
