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
});
