// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Once a project has loaded, a failed save must stay visible INSIDE the loaded
// editor (issue #928): the old shell appended save failures to serviceErrors,
// which only the pre-load placeholder branch rendered, so a user whose session
// expired or whose project changed in another tab kept editing against silent
// autosave failures and lost work. These tests drive the loaded shell's onSave
// with classified failures and assert the persistent banner: per-reason recovery
// actions, persistence across re-renders and further edits, the 409 autosave
// suppression, clearing on a subsequent successful save, and that the editor
// (the user's in-memory work) stays mounted underneath throughout.
//
// The heavyweight <Editor> is mocked with a stub that captures the props
// HostedWebEditor hands it and renders a marker, so the tests can invoke onSave
// directly and assert the editor remains rendered without booting WASM.

import { describe, test, expect, beforeEach, afterEach, rs } from '@rstest/core';
import type { Mock, MockInstance } from '@rstest/core';

import * as React from 'react';
import { render, screen, act, fireEvent } from '@testing-library/react';

import { fromUint8Array } from '@simlin/core/base64';
import type { ProtobufProjectData } from '../Editor';
import * as core from '../hosted-web-editor-core';

interface CapturedEditorProps {
  onSave: (project: ProtobufProjectData, currVersion: number) => Promise<number | undefined>;
  initialProjectVersion: number;
}

let captured: CapturedEditorProps | undefined;

rs.mock('../Editor', () => ({
  __esModule: true,
  Editor: (p: CapturedEditorProps) => {
    captured = p;
    return React.createElement('div', { 'data-testid': 'editor-stub' });
  },
}));

// rs.mock is hoisted above the imports, so HostedWebEditor binds to the stub
// Editor when it is imported here.
import { HostedWebEditor } from '../HostedWebEditor';

function loadedResponse(version: number): Response {
  const pb = fromUint8Array(new Uint8Array([1, 2, 3]));
  return { status: 200, json: async () => ({ pb, version }) } as unknown as Response;
}

function jsonResponse(status: number, body: unknown): Response {
  return { status, json: async () => body } as unknown as Response;
}

function makeProject(): ProtobufProjectData {
  return { data: new Uint8Array([9, 9, 9]) } as unknown as ProtobufProjectData;
}

async function flushDeferredLoad(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  });
}

function countPosts(fetchMock: Mock): number {
  return fetchMock.mock.calls.filter((c) => (c[1] as RequestInit | undefined)?.method === 'POST').length;
}

// Render the shell with an injected fetch and drive it past the deferred load so
// the (stubbed) Editor mounts and `captured` holds the wired-up onSave.
async function renderLoaded(
  fetchImpl: (input: string, init?: RequestInit) => Promise<Response>,
): Promise<{ fetchMock: Mock; rerender: () => void }> {
  captured = undefined;
  const fetchMock = rs.fn(fetchImpl);
  (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;
  let result!: ReturnType<typeof render>;
  await act(async () => {
    result = render(<HostedWebEditor username="alice" projectName="climate" baseURL="" />);
  });
  await flushDeferredLoad();
  const rerender = (): void => {
    result.rerender(<HostedWebEditor username="alice" projectName="climate" baseURL="" />);
  };
  return { fetchMock, rerender };
}

// A fetch whose POST behavior is swappable mid-test (load GETs always succeed).
function fetchWithPost(post: () => Promise<Response>): (input: string, init?: RequestInit) => Promise<Response> {
  return async (_input: string, init?: RequestInit) => {
    if (init?.method === 'POST') {
      return post();
    }
    return loadedResponse(5);
  };
}

async function save(currVersion = 5): Promise<number | undefined> {
  let returned: number | undefined;
  await act(async () => {
    returned = await captured!.onSave(makeProject(), currVersion);
  });
  return returned;
}

describe('HostedWebEditor save-failure surface', () => {
  const originalFetch = globalThis.fetch;
  let reloadSpy: MockInstance;
  let signInSpy: MockInstance;

  beforeEach(() => {
    // jsdom's window.location.reload / window.open are non-configurable or
    // unimplemented, so the shell routes both recovery navigations through core
    // seams (like the delete flow's redirectToHome) that a spy can intercept.
    reloadSpy = rs.spyOn(core, 'reloadPage').mockImplementation(() => {});
    signInSpy = rs.spyOn(core, 'openSignInPage').mockImplementation(() => {});
  });

  afterEach(() => {
    (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
    rs.restoreAllMocks();
  });

  test('no failure surface renders while saves succeed', async () => {
    await renderLoaded(fetchWithPost(async () => jsonResponse(200, { version: 6 })));

    await save(5);

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('a 409 shows the conflict surface with its recovery action, keeps the editor mounted, and persists', async () => {
    const { rerender } = await renderLoaded(
      fetchWithPost(async () => jsonResponse(409, { error: 'version conflict' })),
    );

    const returned = await save(5);

    expect(returned).toBeUndefined();
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toMatch(/changed somewhere else/i);
    expect(screen.getByRole('button', { name: /reload and discard/i })).not.toBeNull();
    // The user's in-memory work stays rendered underneath the banner.
    expect(screen.getByTestId('editor-stub')).not.toBeNull();

    // The surface persists across re-renders (it must not be a transient toast).
    await act(async () => {
      rerender();
    });
    expect(screen.getByRole('alert').textContent).toMatch(/changed somewhere else/i);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('the conflict reload action navigates through the reload seam', async () => {
    await renderLoaded(fetchWithPost(async () => jsonResponse(409, { error: 'version conflict' })));
    await save(5);

    fireEvent.click(screen.getByRole('button', { name: /reload and discard/i }));

    expect(reloadSpy).toHaveBeenCalledTimes(1);
  });

  test('after a 409, further autosaves against the same stale version are suppressed', async () => {
    // In practice every autosave after a conflict carries the same currVersion
    // (the editor only learns a new version from a successful save; fractional
    // cache-key drift, issue #958, is the rare exception), so re-POSTing would
    // just 409 again. The shell suppresses exact matches: no request, banner
    // stays. This is the deliberate "don't hammer a known-stale version" policy.
    const { fetchMock } = await renderLoaded(
      fetchWithPost(async () => jsonResponse(409, { error: 'version conflict' })),
    );

    await save(5);
    expect(countPosts(fetchMock)).toBe(1);

    const returned = await save(5);

    expect(returned).toBeUndefined();
    expect(countPosts(fetchMock)).toBe(1);
    expect(screen.getByRole('alert').textContent).toMatch(/changed somewhere else/i);
  });

  test('a 401 shows the session-expired surface whose action opens sign-in in a new tab', async () => {
    await renderLoaded(fetchWithPost(async () => jsonResponse(401, { error: 'unauthorized' })));

    await save(5);

    const alert = screen.getByRole('alert');
    expect(alert.textContent).toMatch(/session expired/i);
    expect(screen.getByTestId('editor-stub')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /sign in/i }));
    // A same-tab navigation would unload the editor and destroy the unsaved
    // in-memory work; the new-tab seam is what keeps it recoverable.
    expect(signInSpy).toHaveBeenCalledTimes(1);
    expect(signInSpy).toHaveBeenCalledWith('/');
  });

  test('a network/server failure shows the transient surface and the next save retries', async () => {
    let failing = true;
    const { fetchMock } = await renderLoaded(
      fetchWithPost(() =>
        failing ? Promise.reject(new Error('connection refused')) : Promise.resolve(jsonResponse(200, { version: 6 })),
      ),
    );

    await save(5);

    const alert = screen.getByRole('alert');
    expect(alert.textContent).toMatch(/not saved/i);
    // Unlike a conflict, a transient failure must NOT suppress the next attempt.
    failing = false;
    const returned = await save(5);
    expect(returned).toBe(6);
    expect(countPosts(fetchMock)).toBe(2);
  });

  test('a subsequently successful save clears the failure surface', async () => {
    let post: () => Promise<Response> = () => Promise.reject(new Error('connection refused'));
    await renderLoaded(fetchWithPost(() => post()));

    await save(5);
    expect(screen.getByRole('alert')).not.toBeNull();

    post = async () => jsonResponse(200, { version: 6 });
    const returned = await save(5);

    expect(returned).toBe(6);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
  });

  test('a save failure never surfaces in read-only mode (save is a no-op)', async () => {
    captured = undefined;
    const fetchMock = rs.fn(async () => loadedResponse(5));
    (globalThis as unknown as { fetch: typeof fetch }).fetch = fetchMock as unknown as typeof fetch;
    await act(async () => {
      render(<HostedWebEditor username="alice" projectName="climate" baseURL="" readOnlyMode={true} />);
    });
    await flushDeferredLoad();

    const returned = await save(5);

    expect(returned).toBeUndefined();
    expect(countPosts(fetchMock as unknown as Mock)).toBe(0);
    expect(screen.queryByRole('alert')).toBeNull();
  });
});
