// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The delayed-auth deep-link scenario (issue #933): the app renders a
// two-segment project URL without waiting on its auth gate (so PUBLIC projects
// embed and open anonymously), which means a PRIVATE project's first GET can
// race the host's session restoration (Firebase identity -> POST /session) and
// fail with a 401 even though the viewer is the owner. The shell must retry the
// load when the host's auth identity improves -- exactly once per identity
// change, driven by the `authenticatedUserId` prop -- and must NOT wait on auth
// for public projects, refetch an already-loaded project, or loop against a
// genuinely-forbidden one.
//
// The heavyweight <Editor> is mocked with a stub marker so the loaded branch is
// observable without booting WASM; fetch counts are the retry-policy assertions.

import { describe, test, expect, afterEach, rs } from '@rstest/core';
import type { Mock } from '@rstest/core';

import * as React from 'react';
import { render, screen, act } from '@testing-library/react';

import { fromUint8Array } from '@simlin/core/base64';

rs.mock('../Editor', () => ({
  __esModule: true,
  Editor: () => React.createElement('div', { 'data-testid': 'editor-stub' }),
}));

// rs.mock is hoisted above the imports, so HostedWebEditor binds to the stub
// Editor when it is imported here.
import { HostedWebEditor } from '../HostedWebEditor';

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function createDeferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function loadedResponse(version = 5): Response {
  const pb = fromUint8Array(new Uint8Array([1, 2, 3]));
  return { status: 200, json: async () => ({ pb, version }) } as unknown as Response;
}

function statusResponse(status: number): Response {
  return { status, json: async () => ({}) } as unknown as Response;
}

function installFetch(fetchMock: Mock): void {
  (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;
}

// The initial load is deferred a macrotask (the StrictMode-safe setTimeout);
// flushing one macrotask inside act() also drains any retry issued from an
// effect or a load continuation.
async function flush(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  });
}

function editorAt(auth: string | undefined): React.ReactElement {
  return (
    <HostedWebEditor
      username="alice"
      projectName="climate"
      baseURL=""
      readOnlyMode={!auth}
      authenticatedUserId={auth}
    />
  );
}

describe('HostedWebEditor auth-recovery retry (#933)', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
  });

  test('retries exactly once when the auth identity improves after an unauthorized load', async () => {
    // Owner deep link: the first GET runs before the server session exists
    // (401); once the host commits the signed-in user the shell re-issues the
    // load and the editor opens with no manual reload.
    let authorized = false;
    const fetchMock = rs.fn(async () => (authorized ? loadedResponse() : statusResponse(401)));
    installFetch(fetchMock);

    const { rerender } = render(editorAt(undefined));
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/open this model/)).not.toBeNull();

    authorized = true;
    await act(async () => {
      rerender(editorAt('alice'));
    });
    await flush();

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();

    // Unrelated re-renders with the same identity must not refetch.
    await act(async () => {
      rerender(editorAt('alice'));
    });
    await act(async () => {
      rerender(editorAt('alice'));
    });
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  test('a public project loads immediately and a later identity change does not refetch', async () => {
    // The embed/anonymous path: no auth wait before the load, and the host's
    // auth state resolving afterwards must not disturb the loaded editor.
    const fetchMock = rs.fn(async () => loadedResponse());
    installFetch(fetchMock);

    const { rerender } = render(editorAt(undefined));
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();

    await act(async () => {
      rerender(editorAt('alice'));
    });
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('a non-owner settles on the error placeholder with no retry loop', async () => {
    // The viewer signs in but still isn't allowed to see the project: one
    // retry for the identity change, then the failure is final.
    const fetchMock = rs.fn(async () => statusResponse(401));
    installFetch(fetchMock);

    const { rerender } = render(editorAt(undefined));
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/open this model/)).not.toBeNull();

    await act(async () => {
      rerender(editorAt('mallory'));
    });
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/open this model/)).not.toBeNull();

    // Re-renders with the unchanged identity never re-issue the load.
    await act(async () => {
      rerender(editorAt('mallory'));
    });
    await flush();
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/open this model/)).not.toBeNull();
  });

  test('a load that fails after the identity already improved retries immediately', async () => {
    // The race the prop-change effect alone cannot see: the 401 lands AFTER the
    // host committed the signed-in user. The load continuation must notice the
    // newer identity and re-issue the request itself.
    const first = createDeferred<Response>();
    const second = createDeferred<Response>();
    const responses = [first.promise, second.promise];
    const fetchMock = rs.fn(() => responses.shift() ?? Promise.reject(new Error('unexpected third fetch')));
    installFetch(fetchMock);

    const { rerender } = render(editorAt(undefined));
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Identity improves while the first load is still in flight: no second
    // request yet (the in-flight attempt's completion owns the decision).
    await act(async () => {
      rerender(editorAt('alice'));
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      first.resolve(statusResponse(401));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    // The immediate retry keeps the loading placeholder up -- no error flash.
    expect(screen.queryByText(/open this model/)).toBeNull();

    await act(async () => {
      second.resolve(loadedResponse());
    });
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('StrictMode with a mount-time identity still issues exactly one load', async () => {
    // The retry effect also runs on mount (and doubled under StrictMode); it
    // must fire on identity CHANGE only, or a signed-in cold load would fetch
    // more than once.
    const fetchMock = rs.fn(async () => loadedResponse());
    installFetch(fetchMock);

    render(<React.StrictMode>{editorAt('alice')}</React.StrictMode>);
    await flush();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('a non-auth-shaped load failure is not retried when the identity changes', async () => {
    // 404 means the user or file genuinely does not exist (the server 401s
    // private projects; see src/server/api.ts) -- signing in cannot fix it.
    const fetchMock = rs.fn(async () => statusResponse(404));
    installFetch(fetchMock);

    const { rerender } = render(editorAt(undefined));
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/open this model/)).not.toBeNull();

    await act(async () => {
      rerender(editorAt('alice'));
    });
    await flush();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/open this model/)).not.toBeNull();
  });
});
