// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// saveProject() POSTs the serialized project and returns a discriminated result.
// Like loadProject, it must NEVER reject: the Editor's save queue treats a
// resolved-undefined as "save failed, retry with the next edit", but a rejection
// used to escape before the failure was even recorded (issue #928) -- a proxy
// HTML error page or an empty body made response.json() throw out of the non-2xx
// branch. Failures are classified (conflict/unauthorized/other) so the shell can
// offer the right recovery. The function is framework-free and calls the global
// `fetch` directly (native fetch throws "Illegal invocation" when called as a
// method of any object but the global), so these tests stub `globalThis.fetch`.

import { describe, test, expect, afterEach, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import { saveProject, ProjectEndpoint } from '../hosted-web-editor-core';
import type { ProtobufProjectData } from '../Editor';

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

// A response whose body is not JSON (proxy error page, empty 502 body):
// response.json() rejects the way fetch's real implementation does.
function nonJsonResponse(status: number): Response {
  return {
    status,
    json: () => Promise.reject(new SyntaxError('Unexpected token < in JSON')),
  } as unknown as Response;
}

const endpoint: ProjectEndpoint = { base: '', username: 'alice', projectName: 'climate' };

const originalFetch = globalThis.fetch;
function installFetch(impl: (input: string, init?: RequestInit) => Promise<Response>): Mock {
  const mock = rs.fn(impl);
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

  test('classifies a 409 as a conflict and carries the server message', async () => {
    installFetch(async () => jsonResponse(409, { error: 'version conflict' }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result).toEqual({ kind: 'error', reason: 'conflict', message: 'version conflict' });
  });

  test('classifies a 401 as unauthorized', async () => {
    // The server's authz middleware answers an expired/cleared session with a
    // 401 {error: 'unauthorized'}; the project route itself 401s with {}.
    installFetch(async () => jsonResponse(401, {}));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('unauthorized');
    }
  });

  test('classifies a 403 as unauthorized', async () => {
    installFetch(async () => jsonResponse(403, { error: 'forbidden' }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result).toEqual({ kind: 'error', reason: 'unauthorized', message: 'forbidden' });
  });

  test('classifies a 500 with a JSON error body as other, keeping the message', async () => {
    installFetch(async () => jsonResponse(500, { error: 'internal error' }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result).toEqual({ kind: 'error', reason: 'other', message: 'internal error' });
  });

  test('returns a status-bearing message when the error response has no body message', async () => {
    installFetch(async () => jsonResponse(500, {}));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
      expect(result.message).toMatch(/500/);
    }
  });

  test('does not reject on a non-JSON error body (proxy HTML, empty body)', async () => {
    installFetch(async () => nonJsonResponse(502));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
      expect(result.message).toMatch(/502/);
    }
  });

  test('does not reject when fetch itself rejects (network failure)', async () => {
    installFetch(() => Promise.reject(new Error('connection refused')));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
      expect(result.message).toContain('connection refused');
    }
  });

  test('does not reject on a malformed 2xx body (non-JSON)', async () => {
    installFetch(async () => nonJsonResponse(200));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
    }
  });

  test('does not reject on a 2xx body missing a numeric version', async () => {
    installFetch(async () => jsonResponse(200, { version: 'seven' }));

    const result = await saveProject(endpoint, makeProject(), 6);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
      expect(result.message).toContain('malformed');
    }
  });
});
