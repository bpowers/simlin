// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mock the Firebase SDK before importing App. InnerApp's lazy state init calls
// getAuth() (pure setup) and its mount effect calls onAuthStateChanged(); we
// need these to be no-ops in jsdom. The onAuthStateChanged mock is the clean
// seam these tests drive the auth flow through: it records the (listener,
// unsubscribe) pair on each subscribe so a test can invoke the captured
// listener directly to simulate a Firebase auth-state change -- exactly what
// the real SDK does from its event hub -- and assert each subscription's
// unsubscribe ran on teardown.
jest.mock(
  '@firebase/app',
  () => ({
    initializeApp: jest.fn(() => ({})),
  }),
  { virtual: true },
);

interface AuthSubscription {
  listener: (user: unknown) => void | Promise<void>;
  unsubscribe: jest.Mock;
}
// Every onAuthStateChanged subscribe pushes one entry; tests read the latest
// listener (the live observer) and assert every entry's unsubscribe fired on
// unmount (catching a leaked earlier StrictMode subscription).
const authSubscriptions: AuthSubscription[] = [];

jest.mock(
  '@firebase/auth',
  () => ({
    getAuth: jest.fn(() => ({})),
    connectAuthEmulator: jest.fn(),
    // Record the listener and a fresh unsubscribe stub per subscribe so tests
    // can drive the auth chain via the captured listener and verify teardown.
    onAuthStateChanged: jest.fn((_auth: unknown, listener: (user: unknown) => void) => {
      const unsubscribe = jest.fn();
      authSubscriptions.push({ listener, unsubscribe });
      return unsubscribe;
    }),
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

// Stub Home/Login/NewUser to keep the test focused on App.tsx behavior. The
// Home stub (1) bumps a module-level mount counter in a mount effect so a test
// can assert wouter does NOT remount Home when InnerApp re-renders (the route
// component must keep a stable identity across InnerApp state changes), (2)
// renders its `user.id` so a test can assert the stable `home` callback still
// reads the CURRENT committed user via `latest`, and (3) renders a Logout
// button wired to its `onLogout` prop so handleLogout can be driven through a
// real rendered affordance rather than an internal handle.
const homeMountCount = { value: 0 };
jest.mock('../Home', () => {
  const React = require('react');
  return {
    __esModule: true,
    default: (props: { user?: { id?: string }; onLogout?: () => void }) => {
      React.useEffect(() => {
        homeMountCount.value += 1;
      }, []);
      return React.createElement(
        'div',
        { 'data-testid': 'home' },
        React.createElement('span', { 'data-testid': 'home-user' }, props.user ? props.user.id : ''),
        React.createElement('button', { 'data-testid': 'logout', onClick: props.onLogout }, 'Logout'),
      );
    },
  };
});

// The Login stub renders its `disabled` prop (= App's authUnknown) so a test
// can observe authUnknown flipping false through the DOM rather than internal
// state.
jest.mock('../Login', () => {
  const React = require('react');
  return {
    Login: (props: { disabled?: boolean; error?: string }) =>
      React.createElement(
        'div',
        { 'data-testid': 'login', 'data-disabled': String(!!props.disabled), 'data-error': props.error ?? '' },
        'Login',
      ),
  };
});

jest.mock('../NewUser', () => {
  const React = require('react');
  return {
    NewUser: () => React.createElement('div', { 'data-testid': 'new-user' }, 'NewUser'),
  };
});

// fetch is called at module load by UserInfoSingleton's constructor and again
// by getUserInfo / handleLogout. Per test we set the /api/user response that
// drives the auth gate (401 = signed out, 200 + user = signed in). userInfo is
// a module-level singleton that caches its first /api/user result and only
// refetches on invalidate(); helpers below reset it via resetUserInfoFetch.
function userResponse(status: number, body: unknown): Response {
  return {
    status,
    async json() {
      return body;
    },
  } as unknown as Response;
}

const fetchMock = jest.fn(async () => userResponse(401, {}));
(globalThis as unknown as { fetch: jest.Mock }).fetch = fetchMock;

import * as fs from 'node:fs';
import * as path from 'node:path';

import * as React from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Router } from 'wouter';
import { memoryLocation } from 'wouter/memory-location';
import { signOut } from '@firebase/auth';
import { App, InnerApp } from '../App';

// App.tsx holds a module-level UserInfoSingleton that caches its first
// /api/user result and only refetches on invalidate(). Across tests that cache
// would bleed (a test that signs a user in leaves it cached for the next test).
// We never reset the module (that would split the React copy and trip "invalid
// hook call"); instead resetUserInfoCache() forces the singleton back to a
// clean signed-out state through the component's real logout/invalidate path.
// It mounts a throwaway InnerApp and waits for the auth gate to settle: if a
// prior test left a user cached, Home renders, and we click its Logout button
// (handleLogout -> userInfo.invalidate() with /api/user -> 401 clears the cached
// user); if the cache is already signed-out, Login renders and there is nothing
// to clear. Unmount and clear the captured subscriptions so the next test
// starts clean.
function makeFirebaseUser(): { getIdToken: jest.Mock } {
  return { getIdToken: jest.fn(async () => 'tok') };
}

async function flushMacrotasks(times = 3): Promise<void> {
  for (let i = 0; i < times; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

async function resetUserInfoCache(): Promise<void> {
  // /api/user -> 401 so a logout's invalidate() (and any getUserInfo) resolves
  // to "no user".
  setFetchRoutes({});
  setLocation('/');
  const { unmount } = render(
    <Router hook={memoryLocation({ path: '/', static: true }).hook}>
      <InnerApp />
    </Router>,
  );
  // Let the mount-effect getUserInfo run, then settle on the auth gate: Home if
  // a prior test left a user cached, Login otherwise.
  await act(async () => {
    await flushMacrotasks();
  });
  await waitFor(() => {
    expect(screen.queryByTestId('home') ?? screen.queryByTestId('login')).not.toBeNull();
  });

  if (screen.queryByTestId('logout')) {
    // Signed in from a prior test: log out to clear the cached user (invalidate
    // refetches /api/user -> 401).
    await act(async () => {
      fireEvent.click(screen.getByTestId('logout'));
      await flushMacrotasks();
    });
  }

  unmount();
  authSubscriptions.length = 0;
}

function setLocation(pathname: string, search: string = '') {
  // jsdom's `window.location` is not directly assignable, but `history.pushState`
  // updates pathname/search/href in-place which is exactly what we want for
  // App.tsx's `window.location.pathname`/`search` reads.
  window.history.pushState({}, '', `${pathname}${search}`);
}

// Render InnerApp at the given path under a memory-location Router.
function renderApp(pathname: string) {
  setLocation(pathname);
  return render(
    <Router hook={memoryLocation({ path: pathname, static: true }).hook}>
      <InnerApp />
    </Router>,
  );
}

// The live auth observer is the most recently subscribed listener.
function latestAuthListener(): (user: unknown) => void | Promise<void> {
  expect(authSubscriptions.length).toBeGreaterThan(0);
  return authSubscriptions[authSubscriptions.length - 1].listener;
}

// Route the global fetch mock by URL + method so a test can express "/api/user
// returns this user", "/session POST/DELETE succeeds", etc. Unmatched calls
// resolve 401. The /api/user route governs what userInfo.invalidate() refetches
// (the singleton only refreshes on invalidate, driven by the auth/login flow).
function setFetchRoutes(routes: { user?: { status: number; body: unknown } }): void {
  const userRoute = routes.user ?? { status: 401, body: {} };
  fetchMock.mockReset();
  fetchMock.mockImplementation(async (input: unknown, init?: { method?: string }) => {
    const url = String(input);
    const method = (init?.method ?? 'GET').toUpperCase();
    if (url.endsWith('/api/user')) {
      return userResponse(userRoute.status, userRoute.body);
    }
    if (url.endsWith('/session')) {
      // POST creates a session, DELETE clears it; both succeed for these tests.
      void method;
      return userResponse(200, {});
    }
    return userResponse(401, {});
  });
}

// The Login stub renders App's authUnknown as data-disabled; "no Login, or
// Login enabled" tells us the mount-effect getUserInfo has run (authUnknown
// committed false), which is the precondition for maybeLogin to proceed.
function loginIsDisabled(): string | null {
  const login = screen.queryByTestId('login');
  return login ? login.getAttribute('data-disabled') : null;
}

// Sign a user in through the real auth/login flow and wait for Home to render.
// Mount InnerApp at '/', wait for the mount-effect getUserInfo to flip
// authUnknown false (so maybeLogin's authIsKnown gate is satisfied), then fire
// the captured Firebase auth listener with a user. That drives
// asyncAuthStateChanged -> maybeLogin -> POST /session (200) ->
// handleUsernameChanged -> userInfo.invalidate() -> getUserInfo, which now
// refetches /api/user (the configured user) and commits it; the Home stub then
// renders. Returns the testing-library result.
async function signIn(userId: string) {
  setFetchRoutes({ user: { status: 200, body: { id: userId } } });
  const result = renderApp('/');
  // Wait for getUserInfo to have run (authUnknown committed false).
  await waitFor(() => {
    expect(loginIsDisabled()).toBe('false');
  });
  await act(async () => {
    await latestAuthListener()(makeFirebaseUser());
    // Flush the awaited /session POST and the setTimeout-deferred
    // handleUsernameChanged (invalidate + getUserInfo).
    await new Promise((r) => setTimeout(r, 0));
    await new Promise((r) => setTimeout(r, 0));
  });
  await waitFor(() => {
    expect(screen.queryByTestId('home')).not.toBeNull();
  });
  return result;
}

// Every test starts from a signed-out singleton cache so a prior test's sign-in
// can't bleed into the next. Runs with real timers (the outer beforeEach
// precedes any inner jest.useFakeTimers()).
beforeEach(async () => {
  await resetUserInfoCache();
});

describe('App routing (Switch first-match semantics)', () => {
  beforeEach(() => {
    setFetchRoutes({});
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

  test('does not remount the Home route component when InnerApp re-renders', async () => {
    // wouter renders <Route component={...}> by component TYPE: if InnerApp
    // recreated the `home` function each render, wouter would unmount and
    // remount Home on every InnerApp state change (re-firing getProjects,
    // losing menu state). The class held a stable bound `this.home`; the
    // function component reproduces that with a useCallback([]). Sign a user in
    // through the real login flow, snapshot Home's mount count, then drive an
    // unrelated state change via the captured auth listener and assert Home's
    // mount effect did NOT run again -- and that the stable `home` callback
    // still renders the CURRENT committed user.
    homeMountCount.value = 0;
    await signIn('alice');

    // Home mounted exactly once during sign-in and shows the committed user.
    expect(homeMountCount.value).toBe(1);
    expect(screen.getByTestId('home-user').textContent).toBe('alice');

    // Drive an unrelated InnerApp re-render through the captured auth listener
    // (null user -> asyncAuthStateChanged sets firebaseIdToken: null -> a
    // committed state change). Home must not remount.
    await act(async () => {
      await latestAuthListener()(null);
      await new Promise((r) => setTimeout(r, 0));
    });

    expect(homeMountCount.value).toBe(1);
    // The stable `home` callback reads current state.user through `latest`, so
    // it still reflects the committed user after the re-render.
    expect(screen.getByTestId('home-user').textContent).toBe('alice');
  });
});

describe('InnerApp auth-state-changed error handling', () => {
  let consoleErrorSpy: jest.SpyInstance;

  beforeEach(() => {
    setFetchRoutes({});
    // The fix logs auth-flow errors via console.error so devs can see them
    // in the browser console; suppress in test output.
    consoleErrorSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    consoleErrorSpy.mockRestore();
  });

  test('handles a rejected getIdToken without producing an unhandled rejection', async () => {
    // Drive the auth flow the way Firebase does: invoke the listener registered
    // with onAuthStateChanged (captured by the mock). This is the clean seam --
    // no component-internal access. A user whose getIdToken rejects must be
    // caught and logged, not escape as an unhandled rejection (test.js installs
    // `process.on('unhandledRejection', err => { throw err })`, so a leaked
    // rejection fails the test).
    const { unmount } = renderApp('/alice/widgets');

    const fakeUser = {
      getIdToken: jest.fn().mockRejectedValue(new Error('token revoked')),
    };

    let threw: unknown = undefined;
    await act(async () => {
      try {
        await latestAuthListener()(fakeUser);
      } catch (e) {
        threw = e;
      }
      // Allow any deferred microtasks to flush.
      await new Promise((r) => setTimeout(r, 10));
    });

    expect(threw).toBeUndefined();
    // The fix surfaces the error via console.error (or another visible sink).
    // We assert at least one error log; the exact message is implementation
    // detail.
    expect(consoleErrorSpy).toHaveBeenCalled();

    unmount();
  });
});

describe('InnerApp mount / unmount lifecycle', () => {
  // InnerApp's side effects -- subscribing to onAuthStateChanged and the
  // deferred getUserInfo() -- belong in the mount effect, not in render. React
  // 18 StrictMode (dev) double-invokes the render phase, so a second InnerApp
  // render is performed and discarded; its mount effect never runs. A
  // render-scheduled getUserInfo() would fire for that discarded render and
  // setState() on something React never committed, and a render-registered
  // onAuthStateChanged observer would keep the discarded render reachable via
  // the firebase auth event hub forever. Doing both in the mount effect (and
  // undoing them in its cleanup) makes the StrictMode mount/unmount/mount cycle
  // subscribe -> unsubscribe -> subscribe and schedule -> cancel -> schedule.
  //
  // These tests render a real InnerApp and observe the effect's behavior
  // through observable surfaces only: the mocked onAuthStateChanged (the
  // captured subscriptions, each with its own unsubscribe stub) and
  // getUserInfo's net effect, which flips authUnknown false. The Login stub
  // renders that authUnknown as data-disabled (see loginIsDisabled), so
  // "getUserInfo ran" is read off the DOM. /api/user returns 401 so getUserInfo
  // resolves to "no user" and the gate keeps rendering Login -- only
  // data-disabled changes.

  beforeEach(() => {
    jest.useFakeTimers();
    setFetchRoutes({});
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  it('defers getUserInfo to the mount effect, not render (StrictMode safety)', async () => {
    renderApp('/');

    // After commit but before timers run, getUserInfo has only been scheduled,
    // not executed: authUnknown is still its initial true, so the Login gate is
    // disabled. If the schedule had happened during render it would already
    // have fired here.
    expect(loginIsDisabled()).toBe('true');

    await act(async () => {
      jest.runAllTimers();
    });

    // The deferred getUserInfo ran and committed authUnknown: false (still
    // signed out -- 401 -- so still Login, now enabled).
    await waitFor(() => {
      expect(loginIsDisabled()).toBe('false');
    });
  });

  it('subscribes per committed mount and tears down every subscription across a StrictMode mount/unmount/mount cycle', async () => {
    setLocation('/');
    // Wrap InnerApp directly in StrictMode (this is exactly what <App> does in
    // production); React then commits the fiber, runs the mount effect, its
    // cleanup, and the mount effect again.
    const { unmount } = render(
      <React.StrictMode>
        <Router hook={memoryLocation({ path: '/', static: true }).hook}>
          <InnerApp />
        </Router>
      </React.StrictMode>,
    );

    // StrictMode's mount/unmount/mount yields exactly two subscriptions, the
    // first torn down by the intervening cleanup.
    expect(authSubscriptions.length).toBe(2);
    // The intervening cleanup already unsubscribed the FIRST subscription; the
    // surviving (second) one is still live.
    expect(authSubscriptions[0].unsubscribe).toHaveBeenCalledTimes(1);
    expect(authSubscriptions[1].unsubscribe).not.toHaveBeenCalled();

    await act(async () => {
      jest.runAllTimers();
    });

    // getUserInfo ran on the surviving mount and committed authUnknown: false.
    await waitFor(() => {
      expect(loginIsDisabled()).toBe('false');
    });

    // On unmount EVERY captured subscription's unsubscribe must have fired --
    // a leaked earlier subscription would leave an unsubscribe uncalled.
    unmount();
    for (const sub of authSubscriptions) {
      expect(sub.unsubscribe).toHaveBeenCalledTimes(1);
    }
  });

  it('tears down the auth-state subscription on unmount', () => {
    setLocation('/');
    const { unmount } = render(
      <Router hook={memoryLocation({ path: '/', static: true }).hook}>
        <InnerApp />
      </Router>,
    );

    // A bare InnerApp mount (no StrictMode) subscribes exactly once; its
    // unsubscribe must fire on unmount.
    expect(authSubscriptions).toHaveLength(1);
    const { unsubscribe } = authSubscriptions[0];
    expect(unsubscribe).not.toHaveBeenCalled();

    unmount();

    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  it('cancels the pending getUserInfo timer on unmount', async () => {
    // /api/user returns a user so that IF the deferred getUserInfo were allowed
    // to run it would commit a user and the gate would render Home -- a clearly
    // observable outcome we can assert never happens after an early unmount.
    setFetchRoutes({ user: { status: 200, body: { id: 'alice' } } });
    setLocation('/');
    const { unmount } = render(
      <Router hook={memoryLocation({ path: '/', static: true }).hook}>
        <InnerApp />
      </Router>,
    );

    // Before unmount the deferred getUserInfo has not fired yet -- authUnknown
    // is still true (the schedule is pending under fake timers), so the Login
    // gate is disabled and Home is absent.
    expect(loginIsDisabled()).toBe('true');
    expect(screen.queryByTestId('home')).toBeNull();

    // Spy on clearTimeout strictly around unmount: the effect cleanup must
    // clear the pending getUserInfo timer. Paired with the surviving-DOM check
    // below (getUserInfo never committed a user), this evidences cancellation.
    const clearTimeoutSpy = jest.spyOn(globalThis, 'clearTimeout');
    unmount();
    expect(clearTimeoutSpy).toHaveBeenCalled();
    clearTimeoutSpy.mockRestore();

    await act(async () => {
      jest.runAllTimers();
    });

    // The cancelled getUserInfo never ran: Home was never rendered, and the
    // auth subscription was also torn down.
    expect(screen.queryByTestId('home')).toBeNull();
    expect(authSubscriptions[0].unsubscribe).toHaveBeenCalledTimes(1);
  });
});

describe('InnerApp logout', () => {
  // Drive logout through the rendered affordance: the Home stub renders a Logout
  // button wired to its onLogout prop (= InnerApp.handleLogout). signIn() logs a
  // user in via the real auth/login flow so Home and its button render; then we
  // click the button.
  beforeEach(() => {
    (signOut as jest.Mock).mockClear();
  });

  test('clears the server session, firebase auth state, and the local user', async () => {
    await signIn('alice');
    fetchMock.mockClear();

    await act(async () => {
      fireEvent.click(screen.getByTestId('logout'));
      // Let handleLogout's awaited steps (DELETE /session, signOut, invalidate)
      // settle.
      await new Promise((r) => setTimeout(r, 0));
    });

    // The server session must be torn down...
    const deleteCall = fetchMock.mock.calls.find(
      (call) => (call as unknown[])[1] && ((call as unknown[])[1] as { method?: string }).method === 'DELETE',
    );
    expect(deleteCall).toBeDefined();
    expect((deleteCall as unknown[])[0]).toBe('/session');
    // ...the firebase client signed out...
    expect(signOut).toHaveBeenCalled();
    // ...and the local user dropped so the auth gate shows Login again.
    await waitFor(() => {
      expect(screen.queryByTestId('home')).toBeNull();
      expect(screen.queryByTestId('login')).not.toBeNull();
    });
  });

  test('a session error is cleared after a successful sign-in and does not resurface after logout', async () => {
    // Regression: state.loginError was never cleared, so a user who hit a
    // /session failure, then signed in successfully (a retry), then logged out
    // would see the stale "couldn't finish signing you in" error on the fresh
    // login screen. The success path (and logout) now clear it.
    let allowSession = false;
    let sessionCreated = false;
    fetchMock.mockReset();
    fetchMock.mockImplementation(async (input: unknown, init?: { method?: string }) => {
      const url = String(input);
      const method = (init?.method ?? 'GET').toUpperCase();
      if (url.endsWith('/api/user')) {
        return sessionCreated ? userResponse(200, { id: 'alice' }) : userResponse(401, {});
      }
      if (url.endsWith('/session')) {
        if (method === 'DELETE') {
          sessionCreated = false;
          return userResponse(200, {});
        }
        // POST: fails until the retry, then creates the session.
        if (!allowSession) {
          return userResponse(500, {});
        }
        sessionCreated = true;
        return userResponse(200, {});
      }
      return userResponse(401, {});
    });
    const consoleSpy = jest.spyOn(console, 'error').mockImplementation(() => {});

    renderApp('/');
    await waitFor(() => {
      expect(loginIsDisabled()).toBe('false');
    });

    // First attempt: /session POST fails -> loginError set and surfaced on Login.
    await act(async () => {
      await latestAuthListener()(makeFirebaseUser());
      await flushMacrotasks();
    });
    await waitFor(() => {
      expect(screen.getByTestId('login').getAttribute('data-error')).toMatch(/finish signing you in/i);
    });

    // Retry: /session POST now succeeds -> user committed, Home renders, and the
    // prior loginError is cleared on the success path.
    allowSession = true;
    await act(async () => {
      await latestAuthListener()(makeFirebaseUser());
      await flushMacrotasks();
    });
    await waitFor(() => {
      expect(screen.queryByTestId('home')).not.toBeNull();
    });

    // Sign out -> back to Login with NO stale session error.
    await act(async () => {
      fireEvent.click(screen.getByTestId('logout'));
      await flushMacrotasks();
    });
    await waitFor(() => {
      expect(screen.queryByTestId('login')).not.toBeNull();
    });
    expect(screen.getByTestId('login').getAttribute('data-error')).toBe('');

    consoleSpy.mockRestore();
    setFetchRoutes({});
  });

  test('still returns to the login screen when every network step rejects', async () => {
    // Every step of handleLogout is best-effort: a transient network failure
    // during DELETE /session or the /api/user cache refresh (userInfo.invalidate
    // can rethrow a pending request's rejection) must neither escape as an
    // unhandled rejection from Home's fire-and-forget onLogout call nor leave
    // the UI stuck signed in.
    await signIn('alice');

    const consoleSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
    // After login, make every subsequent fetch reject.
    fetchMock.mockReset();
    fetchMock.mockRejectedValue(new Error('network down'));

    let threw: unknown;
    await act(async () => {
      try {
        fireEvent.click(screen.getByTestId('logout'));
      } catch (e) {
        threw = e;
      }
      // Let any stray microtasks settle (an unhandled rejection would fail the
      // test via Jest's unhandled-rejection reporting).
      await new Promise((r) => setTimeout(r, 10));
    });

    expect(threw).toBeUndefined();
    // The local user was dropped despite the failures, so the gate shows Login.
    await waitFor(() => {
      expect(screen.queryByTestId('home')).toBeNull();
      expect(screen.queryByTestId('login')).not.toBeNull();
    });

    consoleSpy.mockRestore();
    // Restore a benign fetch for the global afterEach/next-test reset.
    setFetchRoutes({});
  });
});
