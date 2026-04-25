// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';

import { App } from './App';
import type { ListProjectsResponse } from './api';

function makeListFetch(body: ListProjectsResponse): jest.Mock {
  return jest.fn().mockResolvedValue({
    ok: true,
    status: 200,
    json: async () => body,
  });
}

let originalFetch: typeof globalThis.fetch | undefined;

beforeEach(() => {
  sessionStorage.clear();
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
  } else {
    delete (globalThis as Partial<typeof globalThis>).fetch;
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
});
