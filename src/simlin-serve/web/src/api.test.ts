// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { encodeProjectPath, fetchProject, fetchProjects } from './api';
import { TOKEN_STORAGE_KEY } from './launch-token';

let originalFetch: typeof globalThis.fetch | undefined;

beforeEach(() => {
  sessionStorage.clear();
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
  } else {
    delete (globalThis as Partial<typeof globalThis>).fetch;
  }
});

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 400,
    status,
    json: async () => body,
  } as unknown as Response;
}

describe('encodeProjectPath', () => {
  test('encodes individual segments while preserving slashes', () => {
    expect(encodeProjectPath('foo/bar.stmx')).toBe('foo/bar.stmx');
    expect(encodeProjectPath('with space/file.xmile')).toBe('with%20space/file.xmile');
    expect(encodeProjectPath('héllo/wörld.mdl')).toBe(
      `${encodeURIComponent('héllo')}/${encodeURIComponent('wörld.mdl')}`,
    );
  });
});

describe('fetchProjects authorization header', () => {
  test('includes Bearer token when sessionStorage has one', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok-123');
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        projects: [],
        git_available: true,
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await fetchProjects();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    const headers = init?.headers as Record<string, string> | undefined;
    expect(headers?.['Authorization']).toBe('Bearer tok-123');
  });

  test('omits Authorization header when no token is stored', async () => {
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        projects: [],
        git_available: true,
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await fetchProjects();

    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    const headers = (init?.headers ?? {}) as Record<string, string>;
    expect(headers['Authorization']).toBeUndefined();
  });
});

describe('fetchProject authorization header', () => {
  test('includes Bearer token on read requests', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok-xyz');
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await fetchProject('teacup.stmx');

    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    const headers = init?.headers as Record<string, string> | undefined;
    expect(headers?.['Authorization']).toBe('Bearer tok-xyz');
  });
});
