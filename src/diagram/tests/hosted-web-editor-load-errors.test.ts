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
// state, so these tests assert that result directly with an injected fetch.

import { fromUint8Array } from '@simlin/core/base64';

import { loadProject, ProjectEndpoint } from '../hosted-web-editor-core';

function makeEndpoint(fetchImpl: () => Promise<unknown>): ProjectEndpoint {
  return {
    base: 'http://test.invalid',
    username: 'alice',
    projectName: 'climate',
    fetch: fetchImpl as unknown as typeof fetch,
  };
}

describe('loadProject error handling', () => {
  it('surfaces a network-level fetch rejection as an error result', async () => {
    const endpoint = makeEndpoint(() => Promise.reject(new Error('connection refused')));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
    if (result.kind === 'error') {
      expect(result.message).toContain('unable to load');
    }
  });

  it('surfaces a non-JSON response body as an error result', async () => {
    const endpoint = makeEndpoint(async () => ({
      status: 200,
      json: () => Promise.reject(new SyntaxError('Unexpected token < in JSON')),
    }));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
  });

  it('surfaces a response missing pb/version as an error result', async () => {
    const endpoint = makeEndpoint(async () => ({
      status: 200,
      json: async () => ({}),
    }));

    const result = await loadProject(endpoint);

    expect(result.kind).toBe('error');
  });

  it('returns a loaded result for a well-formed response', async () => {
    const pb = new Uint8Array([1, 2, 3]);
    const endpoint = makeEndpoint(async () => ({
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
