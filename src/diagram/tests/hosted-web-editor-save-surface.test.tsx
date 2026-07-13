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
import { ProjectController, type EngineApi } from '../project-controller';
import { makeFakeEngine } from './fake-engine';

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
    // Every autosave after a conflict carries the same currVersion: the
    // controller sends its last server-acknowledged version, which only a
    // successful save can change (render-cache drift no longer leaks into it,
    // issue #958). Re-POSTing would just 409 again, so the shell suppresses
    // exact matches: no request, banner stays. This is the deliberate "don't
    // hammer a known-stale version" policy.
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

  test('a legacy version-0 project loads and saves against currVersion 0 (#960)', async () => {
    // Legacy rows created before the server stamped versions carry the proto3
    // default 0, a legitimate version this branch's server work accepts for
    // saves. The shell's loaded-gate used a falsy check on projectVersion, so
    // a version-0 project never left the loading spinner (#960).
    const postVersions: number[] = [];
    await renderLoaded(async (_input: string, init?: RequestInit) => {
      if (init?.method === 'POST') {
        postVersions.push((JSON.parse(init.body as string) as { currVersion: number }).currVersion);
        return jsonResponse(200, { version: 1 });
      }
      return loadedResponse(0);
    });

    // The editor mounts (no permanent spinner) with the version-0 seed.
    expect(screen.getByTestId('editor-stub')).not.toBeNull();
    expect(captured!.initialProjectVersion).toBe(0);

    // And the optimistic-concurrency check round-trips version 0 honestly.
    const returned = await save(0);
    expect(postVersions).toEqual([0]);
    expect(returned).toBe(1);
  });

  test('local edit drift cannot corrupt the save version across a session recovery (#958)', async () => {
    // The end-to-end #958 scenario, driving a REAL ProjectController against
    // the shell's onSave (wired exactly as Editor.tsx's makeController does).
    // The session expires at server version 5, the user keeps editing (>100
    // content edits -- enough that the fractional render-cache key crosses the
    // next integer), then re-authenticates. The retry save must carry 5 -- the
    // version the server actually holds -- not a locally-drifted 6, which
    // would bogus-409 into the dead-end conflict banner (defeating the #928
    // session-expiry rescue) or, if another session had committed 6, silently
    // overwrite that session's save.
    let signedIn = false;
    let serverVersion = 5;
    const postVersions: number[] = [];
    await renderLoaded(async (_input: string, init?: RequestInit) => {
      if (init?.method === 'POST') {
        const body = JSON.parse(init.body as string) as { currVersion: number };
        postVersions.push(body.currVersion);
        if (!signedIn) {
          return jsonResponse(401, { error: 'unauthorized' });
        }
        // Emulate the server's optimistic-concurrency check: only the version
        // it holds is accepted; anything else conflicts.
        if (body.currVersion !== serverVersion) {
          return jsonResponse(409, { error: 'version conflict' });
        }
        serverVersion += 1;
        return jsonResponse(200, { version: serverVersion });
      }
      return loadedResponse(5);
    });

    const engine = makeFakeEngine();
    const controller = new ProjectController({
      initialProjectVersion: captured!.initialProjectVersion,
      input: { format: 'protobuf', data: new Uint8Array([1]) },
      openProtobuf: async () => engine as unknown as EngineApi,
      openJson: async () => engine as unknown as EngineApi,
      // Read `captured` at call time: the shell recreates onSave per render.
      save: async (project, currVersion) =>
        captured!.onSave({ format: 'protobuf', data: project.data as Uint8Array }, currVersion),
      onError: () => {},
    });
    await act(async () => {
      await controller.openInitialProject();
    });

    // The signed-out stretch: every autosave 401s while the edits pile up.
    await act(async () => {
      for (let i = 0; i < 110; i++) {
        await controller.updateProject(new Uint8Array([1, i]));
      }
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    });
    expect(screen.getByRole('alert').textContent).toMatch(/session expired/i);

    // Re-auth, then one more edit triggers the retry save.
    signedIn = true;
    await act(async () => {
      await controller.updateProject(new Uint8Array([2, 0]));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    });

    expect(postVersions[postVersions.length - 1]).toBe(5);
    // The save succeeded: no conflict banner, the session-expired banner cleared.
    expect(screen.queryByRole('alert')).toBeNull();
    expect(serverVersion).toBe(6);
    await act(async () => {
      await controller.dispose();
    });
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
