// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';

import { EditorHost } from './EditorHost';
import type { GetProjectResponse, JsonProjectData } from '../api';
import { Editor as EditorMock } from '../test-utils/diagram-mock';
import type { ClientWsMessage, UpdatesSocket } from '../ws';

function makeFetchResolving(response: GetProjectResponse, status = 200): jest.Mock {
  return jest.fn().mockResolvedValue({
    ok: status >= 200 && status < 400,
    status,
    json: async () => response,
  });
}

// Stand-in for `UpdatesSocket` that records the frames `EditorHost` would
// have sent. We avoid constructing a real `UpdatesSocket` here because
// `EditorHost` only consumes the `send` method, and a fake keeps the
// component test focused on its own emissions rather than the
// reconnect/parse machinery covered by ws.test.ts.
function makeFakeSocket(): { socket: UpdatesSocket; sent: Array<ClientWsMessage> } {
  const sent: Array<ClientWsMessage> = [];
  const socket = {
    send: (msg: ClientWsMessage) => {
      sent.push(msg);
    },
  } as unknown as UpdatesSocket;
  return { socket, sent };
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
    let result: number | undefined;
    await act(async () => {
      result = await onSave?.(projectData, 0);
    });
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
    await act(async () => {
      await onSave?.(projectData, 0);
    });

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
    await act(async () => {
      await onSave?.({ format: 'json', data: '{}' }, 0);
    });

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

  test('on 422, throws a formatted error containing each validation detail', async () => {
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
        ok: false,
        status: 422,
        json: async () => ({
          error: 'validation_failed',
          details: [
            {
              code: 'unknown_dependency',
              message: 'undefined identifier: bogus',
              modelName: 'main',
              variableName: 'bad',
              kind: 'equation',
            },
            {
              code: 'circular_dependency',
              message: 'depends on itself',
              modelName: 'main',
              variableName: 'loop',
              kind: 'equation',
            },
          ],
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;

    let captured: Error | null = null;
    try {
      await onSave?.({ format: 'json', data: '{}' }, 0);
    } catch (err) {
      captured = err as Error;
    }
    expect(captured).not.toBeNull();
    const msg = captured?.message ?? '';
    expect(msg).toMatch(/save failed/i);
    expect(msg).toContain('unknown_dependency');
    expect(msg).toContain('bad');
    expect(msg).toContain('undefined identifier: bogus');
    expect(msg).toContain('circular_dependency');
    expect(msg).toContain('loop');
    expect(msg).toContain('depends on itself');
  });

  test('on 422 with no details, throws a generic save-failed error', async () => {
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
        ok: false,
        status: 422,
        json: async () => ({ error: 'validation_failed', details: [] }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    const onSave = EditorMock.lastProps?.onSave;
    await expect(onSave?.({ format: 'json', data: '{}' }, 0)).rejects.toThrow(/save failed/i);
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

  test('refetches and remounts the Editor when liveVersion advances past state.version', async () => {
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
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":3}',
          version: 3,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="teacup.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(EditorMock.lastProps?.initialProjectVersion).toBe(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Advance liveVersion: simulates a ProjectChanged WS event for this path.
    rerender(<EditorHost path="teacup.stmx" liveVersion={3} />);

    await waitFor(() => expect(EditorMock.lastProps?.initialProjectVersion).toBe(3));
    expect(EditorMock.lastProps?.initialProjectJson).toBe('{"v":3}');
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  test('does not refetch when liveVersion is less than or equal to state.version (own-save echo)', async () => {
    // Initial GET responds with version 0; we then "save" (driven by the
    // onSave handler) and the server responds with version 1. Once the
    // EditorHost knows about version 1 (via the save's POST response),
    // a subsequent WS echo with liveVersion=1 must NOT trigger a refetch
    // — this is the loop-prevention requirement.
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
        ok: true,
        status: 200,
        json: async () => ({ version: 1, path: 'teacup.stmx' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="teacup.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(EditorMock.lastProps?.initialProjectVersion).toBe(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Drive a save via the Editor's onSave callback (mirrors the Editor
    // handing back the new version after a successful POST). After this,
    // the host's state.version is 1.
    const onSave = EditorMock.lastProps?.onSave;
    await act(async () => {
      const result = await onSave?.({ format: 'json', data: '{"v":0}' }, 0);
      expect(result).toBe(1);
    });

    // The own-save echo arrives over the WS with the same version we
    // already know about. The refetch gate must skip it.
    rerender(<EditorHost path="teacup.stmx" liveVersion={1} />);

    // Give React a chance to run any componentDidUpdate side-effects.
    await act(async () => {
      await Promise.resolve();
    });

    // Total fetch calls remain at the initial GET + the save POST: no
    // third GET was issued in response to the echo.
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  test('liveVersion=0 default does not trigger a refetch on initial render', async () => {
    const fetchMock = jest.fn().mockResolvedValueOnce({
      ok: true,
      status: 200,
      json: async () => ({
        json: '{"v":0}',
        version: 0,
        source_format: 'stmx',
      }),
    });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<EditorHost path="teacup.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    // Single GET — the initial mount fetch. No extra refetch from the
    // 0 liveVersion against state.version 0.
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  test('liveVersion does not trigger a refetch when no path is selected', async () => {
    const fetchMock = jest.fn();
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path={null} liveVersion={0} />);
    rerender(<EditorHost path={null} liveVersion={5} />);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  test('shows a "model was updated on disk" toast when liveSource is disk', async () => {
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
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":1}',
          version: 1,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="teacup.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    // Simulate a disk-source advance: the watcher saw an external edit
    // and pushed a new version. EditorHost should refetch and surface
    // the disk-update toast.
    rerender(<EditorHost path="teacup.stmx" liveVersion={1} liveSource="disk" />);

    await waitFor(() => expect(EditorMock.lastProps?.initialProjectVersion).toBe(1));
    expect(screen.getByRole('status').textContent).toMatch(/updated on disk/i);
  });

  test('does not show the disk toast when liveSource is user', async () => {
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
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":1}',
          version: 1,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="teacup.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    rerender(<EditorHost path="teacup.stmx" liveVersion={1} liveSource="user" />);

    await waitFor(() => expect(EditorMock.lastProps?.initialProjectVersion).toBe(1));
    expect(screen.queryByRole('status')).toBeNull();
  });

  test('disk event for old path does not fire toast after path switch (path-change race)', async () => {
    // Render EditorHost with path A and no live event. Then switch to path B
    // while simultaneously delivering a disk event whose liveVersion=1 was
    // meant for path A. The toast must NOT appear because the event belongs
    // to a path that is no longer active.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":"a"}',
          version: 0,
          source_format: 'stmx',
        }),
      })
      .mockResolvedValueOnce({
        // Fetch for path B.
        ok: true,
        status: 200,
        json: async () => ({
          json: '{"v":"b"}',
          version: 0,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="a.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());

    // Switch to path B AND deliver a disk event for path A in the same
    // render. The componentDidUpdate path checks `prev.path !== this.props.path`
    // first; on a path change it clears the disk notice state and re-loads,
    // so the liveVersion gate is never reached for the old path's event.
    rerender(
      <EditorHost path="b.stmx" liveVersion={1} liveSource="disk" />,
    );

    // Give React time to process the update.
    await act(async () => {
      await Promise.resolve();
    });

    // No toast: the disk event belonged to path A, which is no longer loaded.
    expect(screen.queryByRole('status')).toBeNull();
  });

  test('disk toast auto-dismisses after the timeout', async () => {
    jest.useFakeTimers();
    try {
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
          ok: true,
          status: 200,
          json: async () => ({
            json: '{"v":1}',
            version: 1,
            source_format: 'stmx',
          }),
        });
      globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

      const { rerender } = render(<EditorHost path="teacup.stmx" liveVersion={0} />);
      // Drain the initial GET.
      await act(async () => {
        await Promise.resolve();
      });

      rerender(<EditorHost path="teacup.stmx" liveVersion={1} liveSource="disk" />);
      await act(async () => {
        await Promise.resolve();
      });

      expect(screen.queryByRole('status')).not.toBeNull();

      // Advance past the 5s auto-dismiss window. Use act so React flushes
      // the resulting state update synchronously.
      act(() => {
        jest.advanceTimersByTime(5000);
      });

      expect(screen.queryByRole('status')).toBeNull();
    } finally {
      jest.useRealTimers();
    }
  });

  test('emits projectFocused on mount when a path and socket are provided', async () => {
    globalThis.fetch = makeFetchResolving({
      json: '{}',
      version: 0,
      source_format: 'stmx',
    }) as unknown as typeof globalThis.fetch;

    const { socket, sent } = makeFakeSocket();
    render(<EditorHost path="teacup.stmx" socket={socket} />);

    expect(sent).toEqual([{ type: 'projectFocused', path: 'teacup.stmx' }]);
  });

  test('does not emit projectFocused on mount when no path is selected', () => {
    const { socket, sent } = makeFakeSocket();
    render(<EditorHost path={null} socket={socket} />);

    expect(sent).toEqual([]);
  });

  test('does not throw when no socket is provided (optional prop)', async () => {
    globalThis.fetch = makeFetchResolving({
      json: '{}',
      version: 0,
      source_format: 'stmx',
    }) as unknown as typeof globalThis.fetch;

    expect(() => render(<EditorHost path="teacup.stmx" />)).not.toThrow();
  });

  test('emits projectFocused for the new path when path changes', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ json: '{}', version: 0, source_format: 'stmx' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ json: '{}', version: 0, source_format: 'stmx' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { socket, sent } = makeFakeSocket();
    const { rerender } = render(<EditorHost path="a.stmx" socket={socket} />);
    expect(sent).toEqual([{ type: 'projectFocused', path: 'a.stmx' }]);

    rerender(<EditorHost path="b.stmx" socket={socket} />);
    expect(sent).toEqual([
      { type: 'projectFocused', path: 'a.stmx' },
      { type: 'projectFocused', path: 'b.stmx' },
    ]);
  });

  test('skips refetch when path changes but liveVersion equals serverVersion (rename)', async () => {
    // When the server emits ProjectRenamed, App.tsx swaps `selectedPath`
    // from the old key to the new one but does NOT bump liveVersion (the
    // doc state is identical, just at a new key). EditorHost must
    // recognize the path-change case where liveVersion already matches
    // the version the host is holding, and skip the refetch — otherwise
    // the editor remounts unnecessarily and loses in-flight state.
    const fetchMock = jest.fn().mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({ json: '{"v":0}', version: 0, source_format: 'stmx' }),
    });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="a.stmx" liveVersion={0} />);

    // Wait for the initial GET to resolve so state.serverVersion = 0
    // matches the loaded payload's version.
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(fetchMock).toHaveBeenCalledTimes(1);

    rerender(<EditorHost path="b.stmx" liveVersion={0} />);

    // Allow React to flush the path-change effect.
    await act(async () => {
      await Promise.resolve();
    });

    // No new GET fired. The Editor stays mounted with the same payload;
    // the underlying doc state hasn't changed. Only the display name
    // (passed to the Editor mock) reflects the new path.
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(EditorMock.lastProps?.name).toBe('b.stmx');
  });

  test('refetches normally when path changes and liveVersion is higher than current', async () => {
    // Sanity check the symmetric case: a real path swap to a path with a
    // newer live version triggers the refetch as before. Without this,
    // the rename optimization could mask broken refetch behavior.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ json: '{"v":0}', version: 0, source_format: 'stmx' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({ json: '{"v":7}', version: 7, source_format: 'stmx' }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const { rerender } = render(<EditorHost path="a.stmx" liveVersion={0} />);
    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(fetchMock).toHaveBeenCalledTimes(1);

    rerender(<EditorHost path="b.stmx" liveVersion={7} />);

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(fetchMock.mock.calls[1][0]).toBe('/api/projects/b.stmx');
  });

  test('does not re-emit projectFocused when an unrelated prop changes', async () => {
    globalThis.fetch = makeFetchResolving({
      json: '{}',
      version: 0,
      source_format: 'stmx',
    }) as unknown as typeof globalThis.fetch;

    const { socket, sent } = makeFakeSocket();
    const { rerender } = render(<EditorHost path="a.stmx" socket={socket} liveVersion={0} />);
    expect(sent).toHaveLength(1);

    // Bumping liveVersion shouldn't be misread as a path change.
    rerender(<EditorHost path="a.stmx" socket={socket} liveVersion={1} />);
    expect(sent).toHaveLength(1);
  });

  test('emits projectFocused when path transitions from null to a value', async () => {
    globalThis.fetch = makeFetchResolving({
      json: '{}',
      version: 0,
      source_format: 'stmx',
    }) as unknown as typeof globalThis.fetch;

    const { socket, sent } = makeFakeSocket();
    const { rerender } = render(<EditorHost path={null} socket={socket} />);
    expect(sent).toEqual([]);

    rerender(<EditorHost path="a.stmx" socket={socket} />);
    expect(sent).toEqual([{ type: 'projectFocused', path: 'a.stmx' }]);
  });

  test('passes an onSelectionChanged callback to the Editor', async () => {
    globalThis.fetch = makeFetchResolving({
      json: '{}',
      version: 0,
      source_format: 'stmx',
    }) as unknown as typeof globalThis.fetch;

    const { socket } = makeFakeSocket();
    render(<EditorHost path="a.stmx" socket={socket} />);

    await waitFor(() => expect(EditorMock.lastProps).not.toBeNull());
    expect(typeof EditorMock.lastProps?.onSelectionChanged).toBe('function');
  });

  test('debounces selectionChanged frames and emits the latest idents (AC6.2)', async () => {
    jest.useFakeTimers();
    try {
      globalThis.fetch = makeFetchResolving({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }) as unknown as typeof globalThis.fetch;

      const { socket, sent } = makeFakeSocket();
      render(<EditorHost path="a.stmx" socket={socket} />);

      // Drain the initial mount: clears the projectFocused frame so the
      // subsequent assertions see only selection events.
      await act(async () => {
        await Promise.resolve();
      });
      sent.length = 0;

      const onSelectionChanged = EditorMock.lastProps?.onSelectionChanged;
      expect(onSelectionChanged).toBeDefined();

      onSelectionChanged?.(['a']);
      // Second selection arrives 50ms later; the in-flight timer should
      // be cancelled and rescheduled with the newer idents.
      act(() => {
        jest.advanceTimersByTime(50);
      });
      onSelectionChanged?.(['a', 'b']);

      // 150ms after the SECOND call (so 200ms total) the debounce fires
      // exactly once with the latest payload.
      act(() => {
        jest.advanceTimersByTime(149);
      });
      expect(sent).toEqual([]);
      act(() => {
        jest.advanceTimersByTime(1);
      });

      expect(sent).toEqual([
        { type: 'selectionChanged', path: 'a.stmx', variableIdents: ['a', 'b'] },
      ]);
    } finally {
      jest.useRealTimers();
    }
  });

  test('cancels pending selectionChanged on unmount', async () => {
    jest.useFakeTimers();
    try {
      globalThis.fetch = makeFetchResolving({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }) as unknown as typeof globalThis.fetch;

      const { socket, sent } = makeFakeSocket();
      const { unmount } = render(<EditorHost path="a.stmx" socket={socket} />);

      await act(async () => {
        await Promise.resolve();
      });
      sent.length = 0;

      const onSelectionChanged = EditorMock.lastProps?.onSelectionChanged;
      onSelectionChanged?.(['a']);

      // Tear down before the 150ms debounce fires; the timer must be
      // cleared so no stale send() lands after the host is gone.
      unmount();
      act(() => {
        jest.advanceTimersByTime(500);
      });

      expect(sent).toEqual([]);
    } finally {
      jest.useRealTimers();
    }
  });

  test('selectionChanged uses the current path when emitted', async () => {
    jest.useFakeTimers();
    try {
      globalThis.fetch = makeFetchResolving({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }) as unknown as typeof globalThis.fetch;

      const { socket, sent } = makeFakeSocket();
      render(<EditorHost path="models/teacup.xmile" socket={socket} />);

      await act(async () => {
        await Promise.resolve();
      });
      sent.length = 0;

      EditorMock.lastProps?.onSelectionChanged?.(['teacup_temperature']);
      act(() => {
        jest.advanceTimersByTime(150);
      });

      expect(sent).toEqual([
        {
          type: 'selectionChanged',
          path: 'models/teacup.xmile',
          variableIdents: ['teacup_temperature'],
        },
      ]);
    } finally {
      jest.useRealTimers();
    }
  });

  test('does not crash when onSelectionChanged fires without a socket', async () => {
    jest.useFakeTimers();
    try {
      globalThis.fetch = makeFetchResolving({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }) as unknown as typeof globalThis.fetch;

      render(<EditorHost path="a.stmx" />);

      await act(async () => {
        await Promise.resolve();
      });

      EditorMock.lastProps?.onSelectionChanged?.(['a']);
      act(() => {
        jest.advanceTimersByTime(150);
      });
      // No assertion required — the test passes if no exception is
      // thrown when the optional socket is absent.
    } finally {
      jest.useRealTimers();
    }
  });

  test('sustained burst of 5 events over 500ms emits exactly one frame with the latest idents', async () => {
    // Five selection events arrive at 100ms intervals. The debounce window
    // is 150ms so each event resets the timer. After 500ms total only one
    // frame should be sent, carrying the idents from the final event.
    jest.useFakeTimers();
    try {
      globalThis.fetch = makeFetchResolving({
        json: '{}',
        version: 0,
        source_format: 'stmx',
      }) as unknown as typeof globalThis.fetch;

      const { socket, sent } = makeFakeSocket();
      render(<EditorHost path="a.stmx" socket={socket} />);

      await act(async () => {
        await Promise.resolve();
      });
      sent.length = 0;

      const onSelectionChanged = EditorMock.lastProps?.onSelectionChanged;
      expect(onSelectionChanged).toBeDefined();

      // Fire 5 events at 100ms intervals.
      for (let i = 1; i <= 5; i++) {
        onSelectionChanged?.([`v${i}`]);
        act(() => {
          jest.advanceTimersByTime(100);
        });
      }

      // At this point 500ms have elapsed since the first event and 100ms
      // since the last. The debounce timer (150ms) has not yet fired.
      expect(sent).toEqual([]);

      // Advance past the 150ms debounce window from the last event.
      act(() => {
        jest.advanceTimersByTime(50);
      });

      expect(sent).toHaveLength(1);
      expect(sent[0]).toEqual({ type: 'selectionChanged', path: 'a.stmx', variableIdents: ['v5'] });
    } finally {
      jest.useRealTimers();
    }
  });
});
