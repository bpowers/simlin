/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// HostedWebEditor defers the GET /api/projects/:user/:name that hydrates the
// editor into a mount useEffect (not render), so the first paint is the empty
// placeholder and the request runs exactly once per committed mount. A load that
// resolves after unmount must not setState. These tests render the component with
// an injected fetch that fails the load, so the error branch renders without
// booting the heavyweight Editor, and assert the fetch is issued exactly once and
// the post-unmount guard holds.

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, screen, act } from '@testing-library/react';

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

function errorResponse(): Response {
  return { status: 404, json: async () => ({}) } as unknown as Response;
}

// The load is deferred a macrotask (the class's setTimeout(0)); flush it inside
// act() so the scheduled loadProject() runs and the fetch is issued.
async function flushDeferredLoad(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  });
}

describe('HostedWebEditor mount load', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
  });

  it('issues the project GET exactly once on mount and renders the placeholder until it resolves', async () => {
    const deferred = createDeferred<Response>();
    const fetchMock = jest.fn(() => deferred.promise);
    (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;

    const { container } = render(<HostedWebEditor username="alice" projectName="climate" baseURL="" />);

    // The load is deferred a macrotask, so nothing has fetched at first paint and
    // the first render is the empty placeholder.
    expect(fetchMock).not.toHaveBeenCalled();
    expect(container.querySelector('div')).not.toBeNull();

    // Once the deferred load runs, it issues exactly one GET.
    await flushDeferredLoad();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith('/api/projects/alice/climate');

    // A failed load surfaces its message in place of the editor.
    await act(async () => {
      deferred.resolve(errorResponse());
    });
    expect(screen.getByText(/unable to load/)).not.toBeNull();
  });

  it('issues the project GET exactly once under StrictMode (mount/unmount/mount)', async () => {
    // React 18+ StrictMode (dev) double-invokes the render phase and drives every
    // committed component through mount -> unmount -> mount on the same instance.
    // The load lives in a mount useEffect whose cleanup clears the mounted-ref, so
    // the cycle must still issue exactly one fetch and the load that resolves after
    // the throwaway first mount must not setState. A constructor-scheduled (or
    // otherwise un-guarded) load would fire twice here.
    const deferred = createDeferred<Response>();
    const fetchMock = jest.fn(() => deferred.promise);
    (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;

    const { container } = render(
      <React.StrictMode>
        <HostedWebEditor username="alice" projectName="climate" baseURL="" />
      </React.StrictMode>,
    );

    // StrictMode's throwaway-first-mount timer is cancelled by its cleanup, so
    // after the deferred load runs only the live mount's request has fired.
    await flushDeferredLoad();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock).toHaveBeenCalledWith('/api/projects/alice/climate');

    await act(async () => {
      deferred.resolve(errorResponse());
    });

    // Still exactly one request after the load settled, and the error message
    // rendered once (no double-commit duplicate, no setState-after-unmount throw).
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(screen.getByText(/unable to load/)).not.toBeNull();
    expect(container.querySelectorAll('div').length).toBeGreaterThan(0);
  });

  it('does not throw when the load resolves after unmount', async () => {
    const deferred = createDeferred<Response>();
    const fetchMock = jest.fn(() => deferred.promise);
    (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;

    const { unmount } = render(<HostedWebEditor username="alice" projectName="climate" baseURL="" />);

    // Let the deferred load fire so the fetch is genuinely in flight, then unmount
    // while its promise is still pending.
    await flushDeferredLoad();
    expect(fetchMock).toHaveBeenCalledTimes(1);

    unmount();

    // Resolving after unmount must be a no-op (the mounted-ref guard); React
    // would warn (and the test would surface it) on a setState after unmount.
    await act(async () => {
      deferred.resolve(errorResponse());
    });
  });
});
