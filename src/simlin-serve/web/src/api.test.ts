// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import {
  createProject,
  encodeProjectPath,
  fetchProject,
  fetchProjects,
  saveProject,
  ValidationError,
  VersionConflictError,
} from './api';

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

describe('fetchProjects', () => {
  test('hits /api/projects without an Authorization header', async () => {
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        projects: [],
        git_available: true,
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await fetchProjects();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit | undefined];
    expect(url).toBe('/api/projects');
    const headers = (init?.headers ?? {}) as Record<string, string>;
    expect(headers['Authorization']).toBeUndefined();
  });
});

describe('fetchProject', () => {
  test('hits the encoded path without an Authorization header', async () => {
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await fetchProject('teacup.stmx');

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit | undefined];
    expect(url).toBe('/api/projects/teacup.stmx');
    const headers = (init?.headers ?? {}) as Record<string, string>;
    expect(headers['Authorization']).toBeUndefined();
  });
});

describe('saveProject', () => {
  test('POSTs JSON body and returns the new version + path on 200', async () => {
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        version: 3,
        path: 'teacup.stmx',
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const result = await saveProject('teacup.stmx', '{"models":[]}', 2);

    expect(result).toEqual({ version: 3, path: 'teacup.stmx' });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/projects/teacup.stmx');
    expect(init.method).toBe('POST');
    const headers = init.headers as Record<string, string>;
    expect(headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body as string)).toEqual({
      json: '{"models":[]}',
      version: 2,
    });
  });

  test('encodes the path before POSTing', async () => {
    const fetchMock = jest.fn().mockResolvedValue(
      jsonResponse({
        version: 1,
        path: 'sub dir/has space.xmile',
      }),
    );
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await saveProject('sub dir/has space.xmile', '{}', 0);

    const [url] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/projects/sub%20dir/has%20space.xmile');
  });

  test('throws VersionConflictError on 409 carrying the actual version', async () => {
    globalThis.fetch = jest.fn().mockResolvedValue(
      jsonResponse(
        {
          error: 'version_mismatch',
          expected: 2,
          actual: 5,
        },
        409,
      ),
    ) as unknown as typeof globalThis.fetch;

    await expect(saveProject('a.stmx', '{}', 2)).rejects.toBeInstanceOf(VersionConflictError);
    try {
      await saveProject('a.stmx', '{}', 2);
      throw new Error('expected reject');
    } catch (err) {
      expect(err).toBeInstanceOf(VersionConflictError);
      expect((err as VersionConflictError).actualVersion).toBe(5);
    }
  });

  test('throws ValidationError on 422 carrying the error list', async () => {
    globalThis.fetch = jest.fn().mockResolvedValue(
      jsonResponse(
        {
          error: 'validation_failed',
          details: [
            {
              code: 'unknown_dependency',
              message: 'undefined identifier: bogus',
              modelName: 'main',
              variableName: 'bad',
              kind: 'equation',
            },
          ],
        },
        422,
      ),
    ) as unknown as typeof globalThis.fetch;

    try {
      await saveProject('a.stmx', '{}', 0);
      throw new Error('expected reject');
    } catch (err) {
      expect(err).toBeInstanceOf(ValidationError);
      const ve = err as ValidationError;
      expect(ve.errors).toHaveLength(1);
      expect(ve.errors[0]).toEqual({
        code: 'unknown_dependency',
        message: 'undefined identifier: bogus',
        modelName: 'main',
        variableName: 'bad',
        kind: 'equation',
      });
    }
  });

  test('throws a generic Error on other non-OK statuses', async () => {
    globalThis.fetch = jest
      .fn()
      .mockResolvedValue(jsonResponse({ error: 'forbidden' }, 403)) as unknown as typeof globalThis.fetch;

    await expect(saveProject('a.stmx', '{}', 0)).rejects.toThrow(/403/);
  });
});

describe('createProject', () => {
  test('POSTs JSON body with name+format and returns the response', async () => {
    const fetchMock = jest.fn().mockResolvedValue(jsonResponse({ path: 'foo.stmx', version: 0 }));
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const result = await createProject('foo', 'stmx');

    expect(result).toEqual({ path: 'foo.stmx', version: 0 });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/projects/new');
    expect(init.method).toBe('POST');
    const headers = init.headers as Record<string, string>;
    expect(headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body as string)).toEqual({ name: 'foo', format: 'stmx' });
  });

  test('forwards the optional parent_dir to the server', async () => {
    const fetchMock = jest.fn().mockResolvedValue(jsonResponse({ path: 'sub/foo.stmx', version: 0 }));
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    await createProject('foo', 'stmx', 'sub');
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect(JSON.parse(init.body as string)).toEqual({
      name: 'foo',
      format: 'stmx',
      parent_dir: 'sub',
    });
  });

  test('throws an Error carrying the server message on non-OK responses', async () => {
    globalThis.fetch = jest
      .fn()
      .mockResolvedValue(jsonResponse({ error: 'already_exists' }, 409)) as unknown as typeof globalThis.fetch;

    await expect(createProject('dup', 'stmx')).rejects.toThrow(/already_exists|409/i);
  });
});
