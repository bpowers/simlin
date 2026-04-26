// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';

import { NewProjectButton } from './NewProjectButton';
import type { CreateProjectResponse } from '../api';

let originalFetch: typeof globalThis.fetch | undefined;

beforeEach(() => {
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  if (originalFetch) {
    globalThis.fetch = originalFetch;
  } else {
    delete (globalThis as Partial<typeof globalThis>).fetch;
  }
});

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 400,
    status,
    json: async () => body,
  } as unknown as Response;
}

describe('NewProjectButton', () => {
  test('renders a collapsed "+ New model" trigger button', () => {
    render(<NewProjectButton onCreated={() => {}} />);
    expect(screen.queryByRole('button', { name: /new model/i })).not.toBeNull();
    expect(screen.queryByPlaceholderText(/filename/i)).toBeNull();
  });

  test('expands an inline form when clicked', () => {
    render(<NewProjectButton onCreated={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    expect(screen.queryByPlaceholderText(/filename/i)).not.toBeNull();
    expect(screen.queryByRole('button', { name: /^create$/i })).not.toBeNull();
  });

  test('disables Create button when name is empty', () => {
    render(<NewProjectButton onCreated={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    const createBtn = screen.getByRole('button', { name: /^create$/i }) as HTMLButtonElement;
    expect(createBtn.disabled).toBe(true);
  });

  test('shows a client-side error for an invalid name without calling fetch', () => {
    const fetchMock = jest.fn();
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;
    render(<NewProjectButton onCreated={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    const input = screen.getByPlaceholderText(/filename/i) as HTMLInputElement;
    fireEvent.change(input, { target: { value: '../etc' } });

    const createBtn = screen.getByRole('button', { name: /^create$/i }) as HTMLButtonElement;
    expect(createBtn.disabled).toBe(true);
    expect(screen.queryByRole('alert')).not.toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  test('calls onCreated with the server-returned path on a successful create', async () => {
    const response: CreateProjectResponse = { path: 'foo.stmx', version: 0 };
    const fetchMock = jest.fn().mockResolvedValue(jsonResponse(response));
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const calls: Array<string> = [];
    render(<NewProjectButton onCreated={(p) => calls.push(p)} />);

    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    fireEvent.change(screen.getByPlaceholderText(/filename/i), { target: { value: 'foo' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^create$/i }));
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/projects/new');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body as string) as { name: string; format: string };
    expect(body).toEqual({ name: 'foo', format: 'stmx' });
    expect(calls).toEqual(['foo.stmx']);
  });

  test('respects the format dropdown selection', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValue(jsonResponse({ path: 'foo.sd.json', version: 0 }));
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    render(<NewProjectButton onCreated={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    fireEvent.change(screen.getByPlaceholderText(/filename/i), { target: { value: 'foo' } });
    fireEvent.change(screen.getByLabelText(/format/i), { target: { value: 'sd_json' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^create$/i }));
    });

    const init = fetchMock.mock.calls[0][1] as RequestInit;
    const body = JSON.parse(init.body as string) as { name: string; format: string };
    expect(body.format).toBe('sd_json');
  });

  test('renders a server error message when fetch returns 409', async () => {
    const fetchMock = jest
      .fn()
      .mockResolvedValue(jsonResponse({ error: 'already_exists' }, 409));
    globalThis.fetch = fetchMock as unknown as typeof globalThis.fetch;

    const calls: Array<string> = [];
    render(<NewProjectButton onCreated={(p) => calls.push(p)} />);
    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    fireEvent.change(screen.getByPlaceholderText(/filename/i), { target: { value: 'foo' } });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^create$/i }));
    });

    expect(calls).toEqual([]);
    expect(screen.queryByText(/already exists|already_exists/i)).not.toBeNull();
  });

  test('Cancel button collapses the form back to just the trigger', () => {
    render(<NewProjectButton onCreated={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: /new model/i }));
    expect(screen.queryByPlaceholderText(/filename/i)).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /cancel/i }));
    expect(screen.queryByPlaceholderText(/filename/i)).toBeNull();
    expect(screen.queryByRole('button', { name: /new model/i })).not.toBeNull();
  });
});
