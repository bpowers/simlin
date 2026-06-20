/**
 * @jest-environment node
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Regression tests for loadProject() error handling. The deferred load in
// HostedWebEditor is fire-and-forget, so loadProject must never reject: a network
// error, a non-JSON body, or a response missing pb/version used to escape as an
// unhandled rejection and leave the editor permanently blank. loadProject now
// returns a discriminated result (loaded | error) instead of mutating component
// state, so these tests assert that result directly. The core calls the global
// `fetch` (not an injected one -- native fetch throws "Illegal invocation" when
// called as a method of any object but the global), so the tests stub it.

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
