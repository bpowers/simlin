/**
 * @jest-environment jsdom
 *
 * Copyright 2026 The Simlin Authors. All rights reserved.
 * Use of this source code is governed by the Apache License,
 * Version 2.0, that can be found in the LICENSE file.
 */

// Shell-glue tests for HostedWebEditor: the wiring between the functional core
// (hosted-web-editor-core.ts, tested directly elsewhere) and React state /
// navigation. These cover the branches the core tests cannot reach because they
// live in the component:
//  - the readOnlyMode early-return guards in handleSave/handleDelete (a host that
//    cannot mutate must never POST or DELETE),
//  - handleSave committing the new server version into state (the return value
//    fed back to the Editor's save queue),
//  - delete-success triggering a full navigation to the project list.
//
// The heavyweight <Editor> is mocked with a stub that captures the props
// HostedWebEditor hands it, so the test can invoke onSave / onDeleteProject /
// readOnlyMode directly without booting the real editor (WASM/engine).

import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, act } from '@testing-library/react';

import { fromUint8Array } from '@simlin/core/base64';
import type { ProtobufProjectData } from '../Editor';
import * as core from '../hosted-web-editor-core';

// Capture the props HostedWebEditor passes into <Editor>.
interface CapturedEditorProps {
  onSave: (project: ProtobufProjectData, currVersion: number) => Promise<number | undefined>;
  onDeleteProject?: () => Promise<void>;
  readOnlyMode?: boolean;
  initialProjectVersion: number;
}

let captured: CapturedEditorProps | undefined;

jest.mock('../Editor', () => ({
  __esModule: true,
  // Minimal stub: record the props and render a marker so the loaded branch is
  // distinguishable from the placeholder.
  Editor: (p: CapturedEditorProps) => {
    captured = p;
    return null;
  },
}));

// jest.mock is hoisted above the imports, so HostedWebEditor binds to the stub
// Editor when it is imported here.
import { HostedWebEditor } from '../HostedWebEditor';

function loadedResponse(version: number): Response {
  const pb = fromUint8Array(new Uint8Array([1, 2, 3]));
  return { status: 200, json: async () => ({ pb, version }) } as unknown as Response;
}

function makeProject(): ProtobufProjectData {
  return { data: new Uint8Array([9, 9, 9]) } as unknown as ProtobufProjectData;
}

async function flushDeferredLoad(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  });
}

// Render the component with an injected fetch and drive it past the deferred load
// so <Editor> mounts and `captured` holds the wired-up handlers.
async function renderLoaded(
  fetchImpl: (input: string, init?: RequestInit) => Promise<Response>,
  props: { readOnlyMode?: boolean } = {},
): Promise<void> {
  captured = undefined;
  (globalThis as unknown as { fetch: typeof fetch }).fetch = jest.fn(fetchImpl) as unknown as typeof fetch;
  await act(async () => {
    render(<HostedWebEditor username="alice" projectName="climate" baseURL="" readOnlyMode={props.readOnlyMode} />);
  });
  await flushDeferredLoad();
}

describe('HostedWebEditor shell glue', () => {
  const originalFetch = globalThis.fetch;
  let redirectSpy: jest.SpyInstance;

  beforeEach(() => {
    // jsdom's window.location.assign is non-configurable and cannot be spied
    // directly, so the shell routes the post-delete navigation through the core's
    // redirectToHome export, which we intercept here to observe the call without a
    // real page transition.
    redirectSpy = jest.spyOn(core, 'redirectToHome').mockImplementation(() => {});
  });

  afterEach(() => {
    (globalThis as unknown as { fetch: typeof fetch }).fetch = originalFetch;
    jest.restoreAllMocks();
  });

  test('mounts the Editor with the loaded project version once the load resolves', async () => {
    await renderLoaded(async () => loadedResponse(5));

    expect(captured).toBeDefined();
    expect(captured!.initialProjectVersion).toBe(5);
  });

  describe('handleSave', () => {
    test('POSTs and returns the new version, committing it to state', async () => {
      const fetchMock = jest.fn(async (_input: string, init?: RequestInit) => {
        if (init?.method === 'POST') {
          return { status: 200, json: async () => ({ version: 8 }) } as unknown as Response;
        }
        return loadedResponse(5);
      });
      await renderLoaded(fetchMock);

      let returned: number | undefined;
      await act(async () => {
        returned = await captured!.onSave(makeProject(), 5);
      });

      expect(returned).toBe(8);
      const postCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'POST');
      expect(postCall).toBeDefined();
      // setProjectVersion(8) committed: re-rendering Editor sees the new version.
      expect(captured!.initialProjectVersion).toBe(8);
    });

    test('is a no-op (no POST, returns undefined) in read-only mode', async () => {
      const fetchMock = jest.fn(async () => loadedResponse(5));
      await renderLoaded(fetchMock, { readOnlyMode: true });

      let returned: number | undefined = 1;
      await act(async () => {
        returned = await captured!.onSave(makeProject(), 5);
      });

      expect(returned).toBeUndefined();
      const postCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'POST');
      expect(postCall).toBeUndefined();
    });
  });

  describe('handleDelete', () => {
    test('DELETEs and navigates to the project list on success', async () => {
      const fetchMock = jest.fn(async (_input: string, init?: RequestInit) => {
        if (init?.method === 'DELETE') {
          return { status: 200, json: async () => ({}) } as unknown as Response;
        }
        return loadedResponse(5);
      });
      await renderLoaded(fetchMock);

      // In a deletable (not read-only) host the Editor receives the delete handler.
      expect(captured!.onDeleteProject).toBeDefined();
      await act(async () => {
        await captured!.onDeleteProject!();
      });

      const deleteCall = fetchMock.mock.calls.find((c) => (c[1] as RequestInit | undefined)?.method === 'DELETE');
      expect(deleteCall).toBeDefined();
      expect(redirectSpy).toHaveBeenCalledWith('/');
    });

    test('does not pass a delete handler to the Editor in read-only mode', async () => {
      await renderLoaded(async () => loadedResponse(5), { readOnlyMode: true });

      // The shell only forwards onDeleteProject when not read-only, so a read-only
      // host can never reach the DELETE path through the Editor at all.
      expect(captured!.readOnlyMode).toBe(true);
      expect(captured!.onDeleteProject).toBeUndefined();
    });
  });
});
