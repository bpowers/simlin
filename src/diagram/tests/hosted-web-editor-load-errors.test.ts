// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Regression tests for loadProject() error handling. The deferred load in
// HostedWebEditor is fire-and-forget, so loadProject must never reject: a network
// error, a non-JSON body, or a response missing pb/version used to escape as an
// unhandled rejection and leave the editor permanently blank. loadProject now
// returns a discriminated result (loaded | error) instead of mutating component
// state, so these tests assert that result directly. The core calls the global
// `fetch` (not an injected one -- native fetch throws "Illegal invocation" when
// called as a method of any object but the global), so the tests stub it.

import { describe, it, expect, afterEach } from '@rstest/core';

import { fromUint8Array } from '@simlin/core/base64';

import { loadProject, ProjectEndpoint } from '../hosted-web-editor-core';

const endpoint: ProjectEndpoint = { base: 'http://test.invalid', username: 'alice', projectName: 'climate' };

const originalFetch = globalThis.fetch;
function installFetch(impl: () => Promise<unknown>): void {
  (globalThis as unknown as { fetch: typeof fetch }).fetch = impl as unknown as typeof fetch;
}
afterEach(() => {
  (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
});

describe('loadProject error handling', () => {
  it('surfaces a network-level fetch rejection as an error result', async () => {
    installFetch(() => Promise.reject(new Error('connection refused')));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.message).toContain('unable to load');
    }
  });

  it('surfaces a non-JSON response body as an error result', async () => {
    installFetch(async () => ({
      status: 200,
      json: () => Promise.reject(new SyntaxError('Unexpected token < in JSON')),
    }));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
  });

  it('surfaces a response missing pb/version as an error result', async () => {
    installFetch(async () => ({
      status: 200,
      json: async () => ({}),
    }));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
  });

  it('returns a loaded result for a well-formed response', async () => {
    const pb = new Uint8Array([1, 2, 3]);
    installFetch(async () => ({
      status: 200,
      json: async () => ({ pb: fromUint8Array(pb), version: 4 }),
    }));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('loaded');
    if (result.kind === 'loaded') {
      expect(result.projectBinary).toEqual(pb);
      expect(result.projectVersion).toBe(4);
    }
  });
});

describe('loadProject failure classification (#933)', () => {
  // The shell retries an `unauthorized` load when its host's auth identity
  // improves (a deep link to a private project races the session re-mint), so
  // the classification must match what the server actually answers: 401 for a
  // private project without a live owner session (403 is grouped with it for
  // symmetry with saveProject), 404 for a nonexistent user or file -- which no
  // amount of signing in can fix.
  async function reasonFor(status: number): Promise<string | undefined> {
    installFetch(async () => ({ status, json: async () => ({}) }));
    const result = await loadProject(endpoint);
    expect(result.kind).toBe('error');
    return result.kind === 'error' ? result.reason : undefined;
  }

  it('classifies a 401 as unauthorized', async () => {
    expect(await reasonFor(401)).toBe('unauthorized');
  });

  it('classifies a 403 as unauthorized', async () => {
    expect(await reasonFor(403)).toBe('unauthorized');
  });

  it('classifies a 404 as other', async () => {
    expect(await reasonFor(404)).toBe('other');
  });

  it('classifies a 500 as other', async () => {
    expect(await reasonFor(500)).toBe('other');
  });

  it('classifies a network-level failure as other', async () => {
    installFetch(() => Promise.reject(new Error('connection refused')));
    const result = await loadProject(endpoint);
    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
    }
  });

  it('classifies a malformed 2xx body as other', async () => {
    installFetch(async () => ({ status: 200, json: async () => ({}) }));
    const result = await loadProject(endpoint);
    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.reason).toBe('other');
    }
  });
});
