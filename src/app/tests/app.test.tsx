// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mock the Firebase SDK before importing App. The constructor of InnerApp
// calls getAuth() and onAuthStateChanged() at construction time; we need
// these to be no-ops in jsdom.
jest.mock(
  '@firebase/app',
  () => ({
    initializeApp: jest.fn(() => ({})),
  }),
  { virtual: true },
);

jest.mock(
  '@firebase/auth',
  () => ({
    getAuth: jest.fn(() => ({})),
    connectAuthEmulator: jest.fn(),
    // Return a fresh unsubscribe stub per call so the lifecycle tests can
    // assert componentWillUnmount tears the subscription down.
    onAuthStateChanged: jest.fn(() => jest.fn()),
    signOut: jest.fn(async () => {}),
  }),
  { virtual: true },
);

// Stub HostedWebEditor: pulls in Editor.tsx + tons of CSS modules.
jest.mock(
  '@simlin/diagram/HostedWebEditor',
  () => {
    const React = require('react');
    return {
      HostedWebEditor: () => React.createElement('div', { 'data-testid': 'hosted-editor' }, 'Editor'),
    };
  },
  { virtual: true },
);

// Stub Home/Login/NewUser to keep the test focused on App.tsx behavior.
jest.mock('../Home', () => {
  const React = require('react');
  return {
    __esModule: true,
    default: () => React.createElement('div', { 'data-testid': 'home' }, 'Home'),
  };
});

jest.mock('../Login', () => {
  const React = require('react');
  return {
    Login: () => React.createElement('div', { 'data-testid': 'login' }, 'Login'),
  };
});

jest.mock('../NewUser', () => {
  const React = require('react');
  return {
    NewUser: () => React.createElement('div', { 'data-testid': 'new-user' }, 'NewUser'),
  };
});

// fetch is called at module load by UserInfoSingleton's constructor.
const fetchMock = jest.fn(async () => {
  return {
    status: 401,
    async json() {
      return {};
    },
  } as unknown as Response;
});
(globalThis as unknown as { fetch: jest.Mock }).fetch = fetchMock;

import * as fs from 'node:fs';
import * as path from 'node:path';

import * as React from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { Router } from 'wouter';
import { memoryLocation } from 'wouter/memory-location';
import { onAuthStateChanged, signOut } from '@firebase/auth';

import { App, InnerApp } from '../App';

function setLocation(pathname: string, search: string = '') {
  // jsdom's `window.location` is not directly assignable, but `history.pushState`
  // updates pathname/search/href in-place which is exactly what we want for
  // App.tsx's `window.location.pathname`/`search` reads.
  window.history.pushState({}, '', `${pathname}${search}`);
}

describe('App routing (Switch first-match semantics)', () => {
  beforeEach(() => {
    fetchMock.mockClear();
  });

  // Find the body of the JSX <Switch>...</Switch> in App.tsx, stripping
  // line comments so accidental references in `// <Switch>` text don't match.
  function readSwitchBody(): string {
    const sourceRaw = fs.readFileSync(path.join(__dirname, '..', 'App.tsx'), 'utf8');
    // Drop // line comments to avoid false matches on prose like "// <Switch>".
    const source = sourceRaw
      .split('\n')
      .map((line) => {
        const idx = line.indexOf('//');
        return idx === -1 ? line : line.substring(0, idx);
      })
      .join('\n');
    const switchOpen = source.indexOf('<Switch>');
    const switchClose = source.indexOf('</Switch>', switchOpen);
    expect(switchOpen).toBeGreaterThan(-1);
    expect(switchClose).toBeGreaterThan(switchOpen);
    return source.substring(switchOpen, switchClose);
  }

  test('Switch directly contains Route children, not a wrapping div', () => {
    // Structural assertion: wouter's <Switch> uses flattenChildren which only
    // descends into Fragments, not divs. A <div> child has no truthy `path`
    // prop so wouter treats it as a wildcard match: cloneElement(<div>) is
    // returned and Switch's first-match semantics are silently disabled.
    // See node_modules/wouter/src/index.js (flattenChildren / Switch / Route).
    //
    // The fix is to either remove the div from inside Switch, or wrap routes
    // in a Fragment. Either way, Switch's body must not contain a <div>.
    expect(readSwitchBody()).not.toMatch(/<div\b/);
  });

  test('routes /new before /:username/:projectName so the literal wins', () => {
    // Once Switch first-match semantics are restored, the dynamic two-segment
    // route :username/:projectName would shadow any /new variant (well -- it
    // wouldn't here because /new has only one segment, but more importantly:
    // any future overlapping literal would silently double-render). Order
    // routes literal-first as a defensive habit.
    const switchBody = readSwitchBody();
    const newIdx = switchBody.indexOf('path="/new"');
    const dynIdx = switchBody.indexOf('path="/:username/:projectName"');
    expect(newIdx).toBeGreaterThan(-1);
    expect(dynIdx).toBeGreaterThan(-1);
    expect(newIdx).toBeLessThan(dynIdx);
  });

  test('renders the editor at /:user/:project (and not Home)', async () => {
    setLocation('/alice/widgets');
    const { hook } = memoryLocation({ path: '/alice/widgets', static: true });

    render(
      <Router hook={hook}>
        <App />
      </Router>,
    );

    await waitFor(() => {
      expect(screen.queryByTestId('hosted-editor')).not.toBeNull();
    });
    expect(screen.queryByTestId('home')).toBeNull();
  });
});

describe('InnerApp.authStateChanged error handling', () => {
  let consoleErrorSpy: jest.SpyInstance;

  beforeEach(() => {
    fetchMock.mockClear();
    // The fix logs auth-flow errors via console.error so devs can see them
    // in the browser console; suppress in test output.
    consoleErrorSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    consoleErrorSpy.mockRestore();
  });

  test('handles a rejected getIdToken without producing an unhandled rejection', async () => {
    setLocation('/alice/widgets');

    // Render the app to construct an InnerApp instance whose authStateChanged
    // we can call directly. We grab the instance via a ref through a wrapper.
    const ref = React.createRef<InnerApp>();
    const { unmount } = render(
      <Router hook={memoryLocation({ path: '/alice/widgets', static: true }).hook}>
        <InnerApp ref={ref} />
      </Router>,
    );

    expect(ref.current).not.toBeNull();
    const fakeUser = {
      getIdToken: jest.fn().mockRejectedValue(new Error('token revoked')),
    } as unknown as import('@firebase/auth').User;

    // The fix must catch the rejection and log it. Without the fix, this
    // would produce an unhandledRejection that fails the test (test.js
    // installs `process.on('unhandledRejection', err => { throw err })`,
    // and Jest's default behavior is to fail the test on unhandled
    // rejections in promises).
    let threw: unknown = undefined;
    try {
      await ref.current!.authStateChanged(fakeUser);
    } catch (e) {
      threw = e;
    }
    // Allow any deferred microtasks to flush.
    await new Promise((r) => setTimeout(r, 10));

    expect(threw).toBeUndefined();
    // The fix surfaces the error via console.error (or another visible
    // sink). We assert at least one error log; the exact message is
    // implementation detail.
    expect(consoleErrorSpy).toHaveBeenCalled();

    unmount();
  });
});

describe('InnerApp.componentDidMount() / componentWillUnmount() lifecycle', () => {
  // InnerApp's side effects -- subscribing to onAuthStateChanged and the
  // deferred getUserInfo() -- belong in componentDidMount, not the constructor.
  // React 18 StrictMode (dev) double-invokes the render phase, so a second
  // InnerApp instance is created and discarded; its componentDidMount /
  // componentWillUnmount never run. A constructor-scheduled getUserInfo() would
  // fire on that zombie instance and setState() on something React never
  // committed ("Can't call setState on a component that is not yet mounted"),
  // and a constructor-registered onAuthStateChanged observer would keep the
  // zombie reachable via the firebase auth event hub forever. Doing it in
  // componentDidMount (and undoing it in componentWillUnmount) makes the
  // StrictMode mount/unmount/mount cycle subscribe -> unsubscribe -> subscribe
  // and schedule -> cancel -> schedule.

  beforeEach(() => {
    jest.useFakeTimers();
    (onAuthStateChanged as jest.Mock).mockClear();
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  it('does not subscribe to auth state or schedule getUserInfo from the constructor alone', () => {
    const app = new InnerApp({});
    const getUserInfoSpy = jest.spyOn(app, 'getUserInfo').mockResolvedValue(undefined);

    jest.runAllTimers();

    expect(onAuthStateChanged).not.toHaveBeenCalled();
    expect(getUserInfoSpy).not.toHaveBeenCalled();
  });

  it('subscribes and schedules getUserInfo in componentDidMount, exactly once across a StrictMode mount/unmount/mount cycle', () => {
    const app = new InnerApp({});
    const getUserInfoSpy = jest.spyOn(app, 'getUserInfo').mockResolvedValue(undefined);

    app.componentDidMount();
    app.componentWillUnmount();
    app.componentDidMount();
    expect(getUserInfoSpy).not.toHaveBeenCalled();

    jest.runAllTimers();

    expect(getUserInfoSpy).toHaveBeenCalledTimes(1);
    // Two mounts (StrictMode) => two subscriptions; the first is torn down by
    // the intervening componentWillUnmount.
    expect(onAuthStateChanged).toHaveBeenCalledTimes(2);
  });

  it('tears down the auth-state subscription on unmount', () => {
    const app = new InnerApp({});
    jest.spyOn(app, 'getUserInfo').mockResolvedValue(undefined);

    app.componentDidMount();
    const unsubscribe = (onAuthStateChanged as jest.Mock).mock.results[0].value as jest.Mock;
    expect(unsubscribe).not.toHaveBeenCalled();

    app.componentWillUnmount();

    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  it('cancels the pending getUserInfo timer on unmount', () => {
    const app = new InnerApp({});
    const getUserInfoSpy = jest.spyOn(app, 'getUserInfo').mockResolvedValue(undefined);

    app.componentDidMount();
    app.componentWillUnmount();

    jest.runAllTimers();

    expect(getUserInfoSpy).not.toHaveBeenCalled();
  });
});

describe('InnerApp.handleLogout', () => {
  beforeEach(() => {
    fetchMock.mockClear();
  });

  test('clears the server session, firebase auth state, and the local user', async () => {
    setLocation('/');
    const ref = React.createRef<InnerApp>();
    const { unmount } = render(
      <Router hook={memoryLocation({ path: '/', static: true }).hook}>
        <InnerApp ref={ref} />
      </Router>,
    );
    expect(ref.current).not.toBeNull();

    // Seed a signed-in user, as if getUserInfo had succeeded.
    ref.current!.setState({
      user: { id: 'alice' } as unknown as import('../User').User,
      authUnknown: false,
    });
    fetchMock.mockClear();

    await ref.current!.handleLogout();

    // The server session must be torn down...
    const deleteCall = fetchMock.mock.calls.find(
      (call) => (call as unknown[])[1] && ((call as unknown[])[1] as { method?: string }).method === 'DELETE',
    );
    expect(deleteCall).toBeDefined();
    expect((deleteCall as unknown[])[0]).toBe('/session');
    // ...the firebase client signed out...
    expect(signOut).toHaveBeenCalled();
    // ...and the local user dropped so the auth gate shows Login again.
    expect(ref.current!.state.user).toBeUndefined();

    unmount();
  });

  test('still clears the local user when every network step rejects', async () => {
    // Every step of handleLogout is best-effort: a transient network
    // failure during DELETE /session or the /api/user cache refresh
    // (userInfo.invalidate can rethrow a pending request's rejection) must
    // neither escape as an unhandled rejection from Home's fire-and-forget
    // call nor leave the UI stuck signed in.
    setLocation('/');
    const ref = React.createRef<InnerApp>();
    const { unmount } = render(
      <Router hook={memoryLocation({ path: '/', static: true }).hook}>
        <InnerApp ref={ref} />
      </Router>,
    );
    expect(ref.current).not.toBeNull();

    ref.current!.setState({
      user: { id: 'alice' } as unknown as import('../User').User,
      authUnknown: false,
    });

    const consoleSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
    fetchMock.mockClear();
    fetchMock.mockRejectedValue(new Error('network down'));

    let threw: unknown;
    try {
      await ref.current!.handleLogout();
    } catch (e) {
      threw = e;
    }
    // Let any stray microtasks settle (an unhandled rejection would fail
    // the test via Jest's unhandled-rejection reporting).
    await new Promise((r) => setTimeout(r, 10));

    expect(threw).toBeUndefined();
    expect(ref.current!.state.user).toBeUndefined();

    consoleSpy.mockRestore();
    fetchMock.mockReset();
    fetchMock.mockImplementation(
      async () =>
        ({
          status: 401,
          async json() {
            return {};
          },
        }) as unknown as Response,
    );
    unmount();
  });
});
