// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { act, render, screen, waitFor, fireEvent } from '@testing-library/react';

import { App } from './App';
import type { ListProjectsResponse } from './api';
import { TOKEN_STORAGE_KEY } from './launch-token';

function makeListFetch(body: ListProjectsResponse): jest.Mock {
  return jest.fn().mockResolvedValue({
    ok: true,
    status: 200,
    json: async () => body,
  });
}

// Test double for the global WebSocket. We keep it minimal: enough surface
// to record construction and let tests drive incoming messages. The App
// tests here only need to confirm App constructs a socket with the right
// URL and disposes it on unmount; the parsed-message behavior is covered
// by ws.test.ts.
class MockWebSocket {
  static instances: Array<MockWebSocket> = [];

  readonly url: string;
  readyState = 0;
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  closeArgs: { code?: number; reason?: string } | null = null;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  close(code?: number, reason?: string): void {
    this.closeArgs = { code, reason };
    this.readyState = 3;
  }

  emitMessage(data: string): void {
    this.onmessage?.(new MessageEvent('message', { data }));
  }
}

let originalFetch: typeof globalThis.fetch | undefined;
let originalWebSocket: typeof globalThis.WebSocket | undefined;

beforeEach(() => {
  sessionStorage.clear();
  originalFetch = globalThis.fetch;
  originalWebSocket = globalThis.WebSocket;
  MockWebSocket.instances = [];
  globalThis.WebSocket = MockWebSocket as unknown as typeof globalThis.WebSocket;
});

afterEach(() => {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
  } else {
    delete (globalThis as Partial<typeof globalThis>).fetch;
  }
  if (originalWebSocket) {
    globalThis.WebSocket = originalWebSocket;
  } else {
    delete (globalThis as Partial<typeof globalThis>).WebSocket;
  }
});

describe('App shell', () => {
  test('renders empty state when no projects are discovered (AC1.4)', async () => {
    globalThis.fetch = makeListFetch({
      projects: [],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryByText(/no models found/i)).not.toBeNull());
    // Phase 1 explicitly does NOT render a "Create new model" button — that's Phase 8.
    expect(screen.queryByRole('button', { name: /create new model/i })).toBeNull();
  });

  test('renders the git-unavailable banner when git is missing (AC2.5)', async () => {
    globalThis.fetch = makeListFetch({
      projects: [
        {
          path: 'a.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'unavailable' },
          version: 0,
        },
      ],
      git_available: false,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryByRole('banner')).not.toBeNull());
    expect(screen.getByRole('banner').textContent).toMatch(/git not on path/i);
  });

  test('AC2.5 banner is dismissable and the dismissal sticks via sessionStorage', async () => {
    globalThis.fetch = makeListFetch({
      projects: [],
      git_available: false,
    }) as unknown as typeof globalThis.fetch;

    const { unmount } = render(<App />);
    await waitFor(() => expect(screen.queryByRole('banner')).not.toBeNull());

    fireEvent.click(screen.getByRole('button', { name: /dismiss/i }));
    expect(screen.queryByRole('banner')).toBeNull();
    expect(sessionStorage.getItem('simlin-serve-git-hint-dismissed')).toBe('1');

    unmount();
    render(<App />);
    await waitFor(() => expect(screen.queryByText(/no models found/i)).not.toBeNull());
    expect(screen.queryByRole('banner')).toBeNull();
  });

  test('opens an UpdatesSocket against /api/updates with the launch token', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'live-token');
    globalThis.fetch = makeListFetch({
      projects: [],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const url = MockWebSocket.instances[0].url;
    expect(url).toMatch(/\/api\/updates\?token=live-token$/);
  });

  test('does not open a WebSocket when no launch token is stored', async () => {
    globalThis.fetch = makeListFetch({
      projects: [],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    // Allow App's fetch resolution to settle, then assert no socket was
    // constructed. Without a token there's nothing to authenticate, so
    // skipping the connection is the right behavior.
    await waitFor(() => expect(screen.queryByText(/no models found/i)).not.toBeNull());
    expect(MockWebSocket.instances).toHaveLength(0);
  });

  test('closes the WebSocket on unmount', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    globalThis.fetch = makeListFetch({
      projects: [],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    const { unmount } = render(<App />);
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];
    expect(ws.closeArgs).toBeNull();

    unmount();
    expect(ws.closeArgs).not.toBeNull();
  });

  test('drops the entry and clears selection when projectRemoved arrives for the selected path', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    globalThis.fetch = makeListFetch({
      projects: [
        {
          path: 'a.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'untracked' },
          version: 0,
        },
        {
          path: 'b.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'untracked' },
          version: 0,
        },
      ],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(2));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRemoved', path: 'a.stmx' }));
    });

    // The deleted entry is gone from the sidebar.
    await waitFor(() => expect(screen.queryByText('a.stmx')).toBeNull());
    expect(screen.queryByText('b.stmx')).not.toBeNull();
    // Selection cleared, so the editor host renders nothing for selected path.
    // The remaining "b.stmx" entry is unselected (no aria-current).
    const items = screen.getAllByRole('listitem');
    for (const item of items) {
      expect(item.getAttribute('aria-current')).toBeNull();
    }
  });

  test('drops the entry and keeps selection when projectRemoved arrives for a different path', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    globalThis.fetch = makeListFetch({
      projects: [
        {
          path: 'a.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'untracked' },
          version: 0,
        },
        {
          path: 'b.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'untracked' },
          version: 0,
        },
      ],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(2));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRemoved', path: 'b.stmx' }));
    });

    await waitFor(() => expect(screen.queryByText('b.stmx')).toBeNull());
    expect(screen.queryByText('a.stmx')).not.toBeNull();
    // a.stmx remains selected.
    const remaining = screen.getAllByRole('listitem');
    expect(remaining).toHaveLength(1);
    expect(remaining[0].getAttribute('aria-current')).toBe('true');
  });

  test('updates the projects list and selectedPath in place when projectRenamed arrives for the selected path', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    // Three fetches: list, GET for selected path's editor, and a GET for
    // the renamed path's editor (because EditorHost re-mounts on path change).
    // After the rename, the editor uses the new path name when refetching.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          projects: [
            {
              path: 'a.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
            {
              path: 'c.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
          ],
          git_available: true,
        }),
      })
      .mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{}',
          version: 0,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(2));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRenamed', from: 'a.stmx', to: 'b.stmx' }));
    });

    // Sidebar swapped a.stmx for b.stmx; c.stmx untouched.
    await waitFor(() => expect(screen.queryByText('a.stmx')).toBeNull());
    expect(screen.queryByText('b.stmx')).not.toBeNull();
    expect(screen.queryByText('c.stmx')).not.toBeNull();

    // The renamed entry is still selected (carried via path swap).
    const items = screen.getAllByRole('listitem');
    const selected = items.filter((item) => item.getAttribute('aria-current') === 'true');
    expect(selected).toHaveLength(1);
    expect(selected[0].textContent).toMatch(/b\.stmx/);
  });

  test('updates the projects list and keeps the unaffected selection when projectRenamed targets a different path', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          projects: [
            {
              path: 'a.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
            {
              path: 'c.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
          ],
          git_available: true,
        }),
      })
      .mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({
          json: '{}',
          version: 0,
          source_format: 'stmx',
        }),
      });
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(2));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRenamed', from: 'c.stmx', to: 'd.stmx' }));
    });

    // The non-selected entry was renamed.
    await waitFor(() => expect(screen.queryByText('c.stmx')).toBeNull());
    expect(screen.queryByText('d.stmx')).not.toBeNull();
    expect(screen.queryByText('a.stmx')).not.toBeNull();

    // a.stmx is still selected.
    const items = screen.getAllByRole('listitem');
    const selected = items.filter((item) => item.getAttribute('aria-current') === 'true');
    expect(selected).toHaveLength(1);
    expect(selected[0].textContent).toMatch(/a\.stmx/);
  });

  test('carries the live version forward across a projectRenamed for the active path', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    // Two fetches expected at steady state: list, then GET for the
    // selected path's editor. After the disk advance bumps liveVersion
    // beyond serverVersion, a third refetch fires. After the rename
    // (which carries liveVersion forward under the new key), no
    // additional refetch should fire.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          projects: [
            {
              path: 'a.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
          ],
          git_available: true,
        }),
      })
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

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(1));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    // Bump the live version beyond the initial GET's version=0; this
    // forces a refetch and lands the editor on version=3 with a recorded
    // liveVersion of 3.
    await act(async () => {
      ws.emitMessage(
        JSON.stringify({ type: 'projectChanged', path: 'a.stmx', version: 3, source: 'disk' }),
      );
    });

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3));
    const fetchCallsBeforeRename = fetchMock.mock.calls.length;

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRenamed', from: 'a.stmx', to: 'b.stmx' }));
    });

    // The list and selection updated.
    await waitFor(() => expect(screen.queryByText('a.stmx')).toBeNull());
    expect(screen.queryByText('b.stmx')).not.toBeNull();

    // Allow any stray refetch to settle.
    await act(async () => {
      await Promise.resolve();
    });

    // No additional GET should have fired: the liveVersion was carried
    // across, so EditorHost's refetch gate (live > server) does not trip.
    expect(fetchMock.mock.calls.length).toBe(fetchCallsBeforeRename);
  });

  test('falls back to the empty state when the last remaining selected project is removed', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    globalThis.fetch = makeListFetch({
      projects: [
        {
          path: 'only.stmx',
          format: 'stmx',
          mtime: new Date(0).toISOString(),
          size: 0,
          git: { kind: 'untracked' },
          version: 0,
        },
      ],
      git_available: true,
    }) as unknown as typeof globalThis.fetch;

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(1));
    fireEvent.click(screen.getByText('only.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(JSON.stringify({ type: 'projectRemoved', path: 'only.stmx' }));
    });

    await waitFor(() => expect(screen.queryByText(/no models found/i)).not.toBeNull());
  });

  test('surfaces a disk-update toast when the selected project advances via disk', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    // First fetch is the project list. Second is the GET for the
    // selected project (initial mount). Third is the GET refetch
    // triggered by the disk-source live advance.
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          projects: [
            {
              path: 'a.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
          ],
          git_available: true,
        }),
      })
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

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(1));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(
        JSON.stringify({ type: 'projectChanged', path: 'a.stmx', version: 1, source: 'disk' }),
      );
    });

    await waitFor(() => expect(screen.queryByRole('status')).not.toBeNull());
    expect(screen.getByRole('status').textContent).toMatch(/updated on disk/i);
  });

  test('does not show the disk toast when the change came from the user', async () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'tok');
    const fetchMock = jest
      .fn()
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          projects: [
            {
              path: 'a.stmx',
              format: 'stmx',
              mtime: new Date(0).toISOString(),
              size: 0,
              git: { kind: 'untracked' },
              version: 0,
            },
          ],
          git_available: true,
        }),
      })
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

    render(<App />);

    await waitFor(() => expect(screen.queryAllByRole('listitem')).toHaveLength(1));
    fireEvent.click(screen.getByText('a.stmx'));

    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const ws = MockWebSocket.instances[0];

    await act(async () => {
      ws.emitMessage(
        JSON.stringify({ type: 'projectChanged', path: 'a.stmx', version: 1, source: 'user' }),
      );
    });

    // Allow the refetch to settle.
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3));
    expect(screen.queryByRole('status')).toBeNull();
  });
});
