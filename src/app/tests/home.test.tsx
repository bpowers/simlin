// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests for Home: the Logout menu item must actually invoke the logout
// callback (it used to only close the menu, leaving users with no way to
// sign out), and the deferred getProjects() fetch must be StrictMode-safe
// (no constructor side effects), cancel on unmount, and survive a network
// rejection without an unhandled rejection.

// Replace the diagram component library with light passthroughs; we only
// need clickable buttons and a Menu that renders its children when open.
jest.mock(
  '@simlin/diagram',
  () => {
    const React = require('react');
    // eslint-disable-next-line react/display-name
    const Pass = (name: string) => (props: { children?: React.ReactNode }) =>
      React.createElement('div', { 'data-component': name }, props.children);
    const IconButton = ({
      children,
      onClick,
    }: {
      children?: React.ReactNode;
      onClick?: (e: unknown) => void;
    } & Record<string, unknown>) => React.createElement('button', { onClick }, children);
    // A real <button> that forwards onClick (variant/color props stripped) so a
    // test can click Retry; children render so its label is queryable.
    const Button = ({ children, onClick }: { children?: React.ReactNode; onClick?: () => void }) =>
      React.createElement('button', { onClick }, children);
    const Menu = ({ open, children }: { open: boolean; children?: React.ReactNode }) =>
      open ? React.createElement('div', { role: 'menu' }, children) : null;
    const MenuItem = ({ onClick, children }: { onClick?: () => void; children?: React.ReactNode }) =>
      React.createElement('button', { role: 'menuitem', onClick }, children);
    return {
      AppBar: Pass('AppBar'),
      Button,
      CircularProgress: () => React.createElement('div', { role: 'progressbar' }),
      ImageList: Pass('ImageList'),
      ImageListItem: Pass('ImageListItem'),
      IconButton,
      Menu,
      MenuItem,
      Paper: Pass('Paper'),
      Toolbar: Pass('Toolbar'),
      Avatar: Pass('Avatar'),
      AccountCircleIcon: () => React.createElement('span', null, 'account'),
    };
  },
  { virtual: true },
);

jest.mock('../NewProject', () => ({
  NewProject: () => null,
}));

import * as React from 'react';
import { render, fireEvent, screen, act } from '@testing-library/react';

import Home from '../Home';
import { User } from '../User';

const user: User = {
  id: 'alice',
  displayName: 'Alice',
  email: 'alice@example.com',
  photoUrl: undefined,
  provider: 'google',
} as unknown as User;

function mockFetch(impl: () => Promise<unknown>): jest.Mock {
  const mock = jest.fn(impl);
  (globalThis as { fetch?: unknown }).fetch = mock;
  return mock;
}

const okProjects = () =>
  Promise.resolve({
    status: 200,
    json: async () => [],
  });

afterEach(() => {
  delete (globalThis as { fetch?: unknown }).fetch;
  jest.useRealTimers();
});

describe('Home logout', () => {
  it('clicking Logout invokes onLogout and closes the menu', () => {
    jest.useFakeTimers();
    mockFetch(okProjects);
    const onLogout = jest.fn();
    render(<Home user={user} isNewProject={false} onLogout={onLogout} />);

    // Open the account menu (the trailing icon button in the toolbar).
    const buttons = screen.getAllByRole('button');
    fireEvent.click(buttons[buttons.length - 1]);

    fireEvent.click(screen.getByText('Logout'));

    expect(onLogout).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Logout')).toBeNull();
  });
});

describe('Home.getProjects lifecycle', () => {
  it('does not fetch during render and fetches once under StrictMode (StrictMode safety)', async () => {
    jest.useFakeTimers();
    const fetchMock = mockFetch(okProjects);

    // Render under StrictMode, which double-invokes the render phase and runs
    // mount -> unmount -> mount on the committed fiber. The deferred fetch
    // lives in a mount effect (cancelled by its cleanup), not a render side
    // effect, so rendering alone schedules nothing observable until timers run,
    // and the StrictMode mount/unmount/mount nets exactly one surviving fetch.
    render(
      <React.StrictMode>
        <Home user={user} isNewProject={false} onLogout={() => {}} />
      </React.StrictMode>,
    );

    expect(fetchMock).not.toHaveBeenCalled();
    await act(async () => {
      jest.runAllTimers();
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('fetches after mount', async () => {
    jest.useFakeTimers();
    const fetchMock = mockFetch(okProjects);
    render(<Home user={user} isNewProject={false} onLogout={() => {}} />);

    expect(fetchMock).not.toHaveBeenCalled();
    await act(async () => {
      jest.runAllTimers();
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('cancels the deferred fetch when unmounted before it fires', () => {
    jest.useFakeTimers();
    const fetchMock = mockFetch(okProjects);
    const { unmount } = render(<Home user={user} isNewProject={false} onLogout={() => {}} />);

    unmount();
    jest.runAllTimers();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('survives a network rejection without an unhandled rejection', async () => {
    jest.useFakeTimers();
    mockFetch(() => Promise.reject(new Error('offline')));
    const consoleSpy = jest.spyOn(console, 'error').mockImplementation(() => {});

    render(<Home user={user} isNewProject={false} onLogout={() => {}} />);
    await act(async () => {
      jest.runAllTimers();
    });

    expect(consoleSpy).toHaveBeenCalled();
    consoleSpy.mockRestore();
  });
});

describe('Home load states', () => {
  it('shows a loading indicator until the deferred fetch resolves', () => {
    jest.useFakeTimers();
    mockFetch(okProjects);
    render(<Home user={user} isNewProject={false} onLogout={() => {}} />);

    // The fetch is deferred a macrotask, so before timers run the project area
    // shows the spinner rather than a blank page.
    expect(screen.queryByRole('progressbar')).not.toBeNull();
  });

  it('shows the empty onboarding state when the user has no projects', async () => {
    jest.useFakeTimers();
    mockFetch(okProjects); // resolves to []
    render(<Home user={user} isNewProject={false} onLogout={() => {}} />);

    await act(async () => {
      jest.runAllTimers();
    });

    expect(screen.queryByText(/no models yet/i)).not.toBeNull();
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('shows an error with a Retry that refetches on a failed load', async () => {
    jest.useFakeTimers();
    const consoleSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
    const fetchMock = mockFetch(() => Promise.resolve({ status: 500, json: async () => ({}) }));

    render(<Home user={user} isNewProject={false} onLogout={() => {}} />);
    await act(async () => {
      jest.runAllTimers();
    });

    // A failed load is no longer a silent blank page: it shows a message + Retry.
    expect(screen.queryByText(/load your models/i)).not.toBeNull();
    expect(screen.queryByText('Retry')).not.toBeNull();

    // Retry re-issues the fetch; a now-succeeding response clears the error.
    fetchMock.mockImplementation(okProjects);
    fireEvent.click(screen.getByText('Retry'));
    await act(async () => {
      jest.runAllTimers();
    });

    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(screen.queryByText(/load your models/i)).toBeNull();
    consoleSpy.mockRestore();
  });
});
