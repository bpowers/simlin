// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';

import { EditorHost } from './EditorHost';
import type { GetProjectResponse, JsonProjectData } from '../api';
import { Editor as EditorMock } from '../test-utils/diagram-mock';

function makeFetchResolving(response: GetProjectResponse, status = 200): jest.Mock {
  return jest.fn().mockResolvedValue({
    ok: status >= 200 && status < 400,
    status,
    json: async () => response,
  });
}

let originalFetch: typeof globalThis.fetch | undefined;

beforeEach(() => {
  EditorMock.lastProps = null;
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
  } else {
    delete (globalThis as Partial<typeof globalThis>).fetch;
  }
});

describe('EditorHost', () => {
  test('renders nothing when no path is selected', () => {
    const { container } = render(<EditorHost path={null} />);
    expect(container.querySelector('[data-testid="editor-mock"]')).toBeNull();
  });

  test('fetches and renders the Editor for an .stmx file (AC3.1)', async () => {
    const response: GetProjectResponse = {
      json: '{"models":[]}',
      version: 7,
      source_format: 'stmx',
    };
    const fetchMock = makeFetchResolving(response);
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const props = EditorMock.lastProps;
    expect(props?.inputFormat).toBe('json');
    expect(props?.initialProjectJson).toBe('{"models":[]}');
    expect(props?.initialProjectVersion).toBe(7);
    // Phase 2 drops embedded + readOnlyMode so the Editor accepts edits.
    expect(props?.embedded).toBeUndefined();
    expect(props?.readOnlyMode).toBeUndefined();
    expect(props?.name).toBe('teacup.stmx');
    expect(typeof props?.onSave).toBe('function');

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/teacup.stmx');
  });

  test('encodes path segments individually when fetching (AC3.1)', async () => {
    const response: GetProjectResponse = {
      json: '{}',
      version: 0,
      source_format: 'xmile',
    };
    const fetchMock = makeFetchResolving(response);
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="sub dir/has space.xmile" />);

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/sub%20dir/has%20space.xmile');
  });

  test('renders the .mdl sidecar banner when serving from .mdl (AC3.3)', async () => {
    const response: GetProjectResponse = {
      json: '{}',
      version: 0,
      source_format: 'mdl',
    };
    globalThis.fetch = makeFetchResolving(response) as unknown as typeof globalThis.fetch;

    render(<EditorHost path="population.mdl" />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(screen.getByText(/sidecar/i)).not.toBeNull();
  });

  test('renders an error banner on fetch failure', async () => {
    globalThis.fetch = jest.fn().mockResolvedValue({
      ok: false,
      status: 404,
      json: async () => ({ error: 'not found' }),
    }) as unknown as typeof globalThis.fetch;

    render(<EditorHost path="missing.stmx" />);

    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeNull());
    expect(screen.getByRole('alert').textContent).toMatch(/not found|failed/i);
  });

  test('onSave POSTs the project JSON and resolves with the new version', async () => {
    // First call (GET): the initial fetch. Second call (POST): the save.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"models":[]}',
          version: 0,
          source_format: 'stmx',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ version: 1, path: 'teacup.stmx' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;
    expect(onSave).toBeDefined();

    const projectData: JsonProjectData = { format: 'json', data: '{"updated":true}' };
    const result = await onSave?.(projectData, 0);
    expect(result).toBe(1);

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [url, init] = fetchMock.mock.calls[1] as [string, RequestInit];
    expect(url).toBe('/api/projects/teacup.stmx');
    expect(init.method).toBe('POST');
    expect(JSON.parse(init.body as string)).toEqual({
      json: '{"updated":true}',
      version: 0,
    });
  });

  test('onSave invokes onPathRedirect when the server returns a different path', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{}',
          version: 0,
          source_format: 'mdl',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ version: 1, path: 'population.sd.json' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const onPathRedirect = jest.fn();
    render(<EditorHost path="population.mdl" onPathRedirect={onPathRedirect} />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;
    const projectData: JsonProjectData = { format: 'json', data: '{}' };
    await onSave?.(projectData, 0);

    expect(onPathRedirect).toHaveBeenCalledTimes(1);
    expect(onPathRedirect).toHaveBeenCalledWith('population.sd.json');
  });

  test('onSave does not invoke onPathRedirect when the server keeps the same path', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{}',
          version: 0,
          source_format: 'stmx',
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ version: 1, path: 'teacup.stmx' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const onPathRedirect = jest.fn();
    render(<EditorHost path="teacup.stmx" onPathRedirect={onPathRedirect} />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;
    await onSave?.({ format: 'json', data: '{}' }, 0);

    expect(onPathRedirect).not.toHaveBeenCalled();
  });

  test('on 409, refetches GET, invokes onConflict with the latest state, and throws a friendly error (AC3.6)', async () => {
    // 1) initial GET, 2) POST returns 409, 3) refetch GET returns the latest.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":0}',
          version: 0,
          source_format: 'stmx',
        }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 409,
        json: async () => ({
          error: 'version_mismatch',
          expected: 0,
          actual: 5,
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":5}',
          version: 5,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const onConflict = jest.fn();
    render(<EditorHost path="teacup.stmx" onConflict={onConflict} />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;
    const projectData: JsonProjectData = { format: 'json', data: '{"v":0}' };
    await expect(onSave?.(projectData, 0)).rejects.toThrow(/conflict/i);

    expect(onConflict).toHaveBeenCalledTimes(1);
    expect(onConflict).toHaveBeenCalledWith('{"v":5}', 5);

    // The third fetch call is the refetch.
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(fetchMock.mock.calls[2][0]).toBe('/api/projects/teacup.stmx');
  });

  test('on 409 without an onConflict callback, EditorHost re-renders with the latest payload', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":0}',
          version: 0,
          source_format: 'stmx',
        }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 409,
        json: async () => ({
          error: 'version_mismatch',
          expected: 0,
          actual: 9,
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":9}',
          version: 9,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(EditorMock.lastProps?.initialProjectVersion).toBe(0);

    const onSave = EditorMock.lastProps?.onSave;
    await act(async () => {
      await expect(onSave?.({ format: 'json', data: '{"v":0}' }, 0)).rejects.toThrow(/conflict/i);
    });

    // After the conflict, EditorHost must re-render with the refetched payload.
    await waitFor(() => expect(EditorMock.lastProps?.initialProjectVersion).toBe(9));
    expect(EditorMock.lastProps?.initialProjectJson).toBe('{"v":9}');
  });
});
