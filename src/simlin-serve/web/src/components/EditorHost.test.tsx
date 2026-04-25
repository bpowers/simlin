// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, screen, waitFor } from '@testing-library/react';

import { EditorHost } from './EditorHost';
import type { GetProjectResponse } from '../api';
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
    expect(props?.embedded).toBe(true);
    expect(props?.readOnlyMode).toBe(true);
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
});
