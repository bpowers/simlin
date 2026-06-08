// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { initializeApp } from '@firebase/app';
import {
  getAuth,
  connectAuthEmulator,
  onAuthStateChanged,
  signOut,
  Auth as FirebaseAuth,
  User as FirebaseUser,
} from '@firebase/auth';

import { useLocation, Route, RouteComponentProps, Switch, Redirect } from 'wouter';

import { defined } from '@simlin/core/common';
import { HostedWebEditor } from '@simlin/diagram/HostedWebEditor';

import Home from './Home';
import { Login } from './Login';
import { NewUser } from './NewUser';
import { User } from './User';

import styles from './App.module.css';

const config = {
  apiKey: 'AIzaSyConH72HQl9xOtjmYJO9o2kQ9nZZzl96G8',
  authDomain: 'auth.simlin.com',
};
const firebaseApp = initializeApp(config);

interface EditorMatchParams {
  username: string;
  projectName: string;

  readonly [paramName: string | number]: string | undefined;
}

class UserInfoSingleton {
  private resultPromise?: Promise<[User | undefined, number]>;
  private result?: [User | undefined, number];
  constructor() {
    // store this promise; we might race calling get() below, but all racers will
    // await this single fetch result.
    this.fetch();
  }

  private fetch(): void {
    const resultPromise = fetch('/api/user', { credentials: 'same-origin' });
    const worker = async (): Promise<[User | undefined, number]> => {
      try {
        const response = await resultPromise;
        const status = response.status;
        const user = status >= 200 && status < 400 ? await response.json() : undefined;

        return [user, status];
      } catch (err) {
        // A network-level failure means "we don't know the user"; callers
        // already treat a non-2xx status as not-authenticated, so report
        // status 0 the same way. Catching here also matters because the
        // promise can sit unconsumed until the next get() (invalidate()
        // fires a fetch nothing immediately awaits) -- a rejection would
        // surface later as an unhandled rejection.
        console.error('fetching /api/user failed:', err);
        return [undefined, 0];
      }
    };
    this.resultPromise = worker();
  }

  async get(): Promise<[User | undefined, number]> {
    if (this.resultPromise) {
      this.result = await this.resultPromise;
      this.resultPromise = undefined;
    }

    return defined(this.result);
  }

  async invalidate(): Promise<void> {
    this.result = undefined;

    const resultPromise = this.resultPromise;
    this.fetch();

    if (resultPromise) {
      // don't leave the promise un-awaited
      await resultPromise;
    }
  }
}

const userInfo = new UserInfoSingleton();

interface AppState {
  authUnknown: boolean;
  isNewUser?: boolean;
  user?: User;
  auth: FirebaseAuth;
  firebaseIdToken?: string | null;
}

function getBaseURL(): string {
  return '';
}

// The mutable, non-render instance state that lived as class instance fields:
// the pending setTimeout(0) handle for the deferred getUserInfo() and the
// onAuthStateChanged unsubscribe function. Held so the unmount cleanup can
// cancel/tear them down. Set up in the mount effect -- see the comment there.
interface InnerAppRefs {
  getUserInfoTimer: ReturnType<typeof setTimeout> | null;
  authUnsubscribe: (() => void) | null;
}

// The escaped async continuations (authStateChanged, maybeLogin, getUserInfo,
// handleLogout, handleUsernameChanged) read CURRENT props/state through this
// ref, exactly as the class read this.props / this.state at call time rather
// than as captured by a stale render closure.
interface InnerAppLatest {
  state: AppState;
}

// Build the initial auth state. Mirrors the class constructor: getAuth() and
// the emulator connection are pure setup (no observer registered yet), so they
// run in the lazy state initializer. The auth-state subscription and the
// deferred getUserInfo() are wired up in the mount effect -- see there.
function makeInitialState(): AppState {
  const isDevServer = process.env.NODE_ENV === 'development';
  const auth = getAuth(firebaseApp);
  if (isDevServer) {
    connectAuthEmulator(auth, 'http://localhost:9099', { disableWarnings: true });
  }
  return {
    authUnknown: true,
    auth,
  };
}

// Exported so unit tests can render InnerApp directly (production renders it
// only via <App>, which wraps it in <React.StrictMode>). Tests drive the auth
// flow through the clean seam the firebase plumbing already provides -- the
// listener passed to the mocked onAuthStateChanged -- rather than any
// component-internal hook.
//
// Converted from React.PureComponent to a function component. InnerApp takes no
// render-affecting props ({} in the class), so React.memo would never bail out
// on anything and is pointless -- a plain function is the right shape. AppState
// is one useState object with a class-parity merging setState helper.
export function InnerApp(): React.JSX.Element {
  const [state, setStateRaw] = React.useState<AppState>(makeInitialState);

  // Class-parity setState: merges a partial patch onto the previous state,
  // exactly like React.Component's setState.
  const setState = React.useCallback((patch: Partial<AppState>): void => {
    setStateRaw((prev) => ({ ...prev, ...patch }));
  }, []);

  const refs = React.useRef<InnerAppRefs>({ getUserInfoTimer: null, authUnsubscribe: null });

  // Refreshed synchronously every render so escaped async callbacks read
  // current state (the class read this.state, which was always current).
  const latest = React.useRef<InnerAppLatest>(undefined as unknown as InnerAppLatest);
  latest.current = { state };

  // Firebase invokes authStateChanged synchronously from its event hub. The
  // previous setTimeout-around-async pattern made this fire-and-forget: any
  // rejection from getIdToken (revoked token, network failure) or maybeLogin
  // (server `/session` error) was silently dropped. Await directly and
  // surface failures via console.error so they're at least visible in dev
  // tools. Returning the promise lets tests `await` the full chain.
  const authStateChanged = React.useCallback(async (user: FirebaseUser | null): Promise<void> => {
    try {
      await asyncAuthStateChanged(user);
    } catch (err) {
      // We deliberately do NOT setState an error here: this method runs on
      // every auth-state transition (including sign-out), and the app's
      // top-level auth gate will re-render Login if needed. Logging keeps
      // the failure visible without overwriting unrelated UI state.
      console.error('auth state change failed:', err);
    }
    // Empty deps: asyncAuthStateChanged/maybeLogin read all state through
    // `latest`, so a fresh closure each render would behave identically -- a
    // stable callback keeps the mount-effect subscription identity steady.
    // (The repo lint config does not enable react-hooks/exhaustive-deps, so no
    // disable directive is needed.)
  }, []);

  const asyncAuthStateChanged = async (user: FirebaseUser | null) => {
    if (!user) {
      setState({ firebaseIdToken: null });
      return;
    }

    const firebaseIdToken = await user.getIdToken();
    setState({ firebaseIdToken });
    await maybeLogin(undefined, firebaseIdToken);
  };

  async function maybeLogin(authIsKnown = false, firebaseIdToken?: string): Promise<void> {
    authIsKnown = authIsKnown || !latest.current.state.authUnknown;
    if (!authIsKnown) {
      return;
    }

    // if we know the user, we don't need to log in
    const [user] = await userInfo.get();
    if (user) {
      return;
    }

    const idToken = firebaseIdToken ?? latest.current.state.firebaseIdToken;
    if (idToken === null || idToken === undefined) {
      return;
    }

    const bodyContents = {
      idToken,
    };

    const base = getBaseURL();
    const apiPath = `${base}/session`;
    const response = await fetch(apiPath, {
      credentials: 'same-origin',
      method: 'POST',
      cache: 'no-cache',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(bodyContents),
    });

    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      const body = await response.json();
      const errorMsg =
        body && body.error ? (body.error as string) : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
      // appendModelError(errorMsg);
      console.log(`session error: ${errorMsg}`);
      return undefined;
    }

    handleUsernameChanged();
  }

  const getUserInfo = React.useCallback(async (): Promise<void> => {
    const [user, status] = await userInfo.get();
    if (!(status >= 200 && status < 400) || !user) {
      setState({
        authUnknown: false,
      });
      await maybeLogin(true);
      return;
    }
    const isNewUser = user.id.startsWith(`temp-`);
    setState({
      authUnknown: false,
      isNewUser,
      user,
    });
    // Deps are just [setState] (stable): maybeLogin reads all state through
    // `latest`, so it need not be a dep. (The repo lint config does not enable
    // react-hooks/exhaustive-deps, so no disable directive is needed.)
  }, [setState]);

  const handleUsernameChanged = React.useCallback((): void => {
    setTimeout(async () => {
      await userInfo.invalidate();
      await getUserInfo();
    });
  }, [getUserInfo]);

  // Sign the user out: clear the server session cookie, then the Firebase
  // client auth state, then drop the cached/in-memory user so the top-level
  // auth gate renders Login again. Each step is best-effort -- even if the
  // network calls fail we still clear local state rather than leaving the
  // user stuck "logged in" with no way out.
  const handleLogout = React.useCallback(async (): Promise<void> => {
    try {
      await fetch(`${getBaseURL()}/session`, {
        credentials: 'same-origin',
        method: 'DELETE',
        cache: 'no-cache',
      });
    } catch (err) {
      console.error('logout: clearing the server session failed:', err);
    }
    try {
      await signOut(latest.current.state.auth);
    } catch (err) {
      console.error('logout: firebase signOut failed:', err);
    }
    // Drop the local user BEFORE refreshing the cached /api/user info: the
    // UI must return to the login screen even if the refresh fails, and
    // invalidate() can rethrow a pending request's rejection (it awaits any
    // in-flight fetch), which would otherwise escape Home's fire-and-forget
    // call as an unhandled rejection.
    setState({ user: undefined, isNewUser: undefined, firebaseIdToken: null });
    try {
      await userInfo.invalidate();
    } catch (err) {
      console.error('logout: refreshing cached user info failed:', err);
    }
  }, [setState]);

  // Mount / unmount effect (formerly componentDidMount / componentWillUnmount).
  // React 18 StrictMode (dev) double-invokes the render phase (so a second
  // InnerApp render is performed and discarded -- this effect never runs for
  // it) and, on the committed fiber, runs the mount effect -> its cleanup ->
  // the mount effect again without re-running the lazy state initializer.
  // Registering the onAuthStateChanged observer and scheduling getUserInfo()
  // here (and undoing both in the cleanup) keeps a discarded render from
  // setState()ing on something React never committed and from being pinned
  // alive by the firebase auth event hub, and makes the StrictMode cycle
  // subscribe -> unsubscribe -> subscribe / schedule -> cancel -> schedule
  // rather than leaking the first of each.
  React.useEffect(() => {
    const r = refs.current;
    r.authUnsubscribe = onAuthStateChanged(latest.current.state.auth, authStateChanged);
    r.getUserInfoTimer = setTimeout(getUserInfo);
    return () => {
      if (r.authUnsubscribe) {
        r.authUnsubscribe();
        r.authUnsubscribe = null;
      }
      if (r.getUserInfoTimer !== null) {
        clearTimeout(r.getUserInfoTimer);
        r.getUserInfoTimer = null;
      }
    };
    // Empty deps: this effect mirrors componentDidMount/Unmount. Everything it
    // reads goes through `latest`/`refs`, and authStateChanged/getUserInfo are
    // stable useCallbacks, so nothing here closes over stale values. (The repo
    // lint config does not enable react-hooks/exhaustive-deps, so no disable
    // directive is needed.)
  }, []);

  // The two route components MUST keep a stable identity across InnerApp
  // re-renders. wouter renders <Route component={...}> by component TYPE, so a
  // fresh function identity each render would unmount and remount Home/the
  // editor on every InnerApp state change -- re-firing Home's getProjects and
  // discarding its menu state. The class's bound `this.home`/`this.editor`
  // arrow fields were stable references that read `this.state` at render time;
  // these useCallback([])s reproduce that exactly by reading current state
  // through `latest` rather than closing over a render's `state`.
  const editor = React.useCallback((editorProps: RouteComponentProps<EditorMatchParams>) => {
    const { username, projectName } = editorProps.params;
    const user = latest.current.state.user;
    const readOnlyMode = !user || user.id !== username;

    return (
      <HostedWebEditor
        username={username}
        projectName={projectName}
        baseURL={getBaseURL()}
        readOnlyMode={readOnlyMode}
      />
    );
  }, []);

  // Rendered via wouter's <Route component={...}>, so it is a real component and
  // may call hooks -- the class relied on exactly this to use useLocation().
  const home = React.useCallback(
    (_props: RouteComponentProps) => {
      const location = useLocation()[0];

      const isNewProject = location === '/new';
      return <Home isNewProject={isNewProject} user={defined(latest.current.state.user)} onLogout={handleLogout} />;
    },
    [handleLogout],
  );

  const urlParams = new URLSearchParams(window.location.search);
  const projectParam = urlParams.get('project');
  if (projectParam) return <Redirect to={projectParam} />;

  // if a user is navigating to a project,
  // skip the high level auth check, to enable public models
  if (!/\/.*\/.*/.test(window.location.pathname)) {
    if (!state.user) {
      return <Login disabled={state.authUnknown} auth={state.auth} />;
    }

    if (state.isNewUser) {
      return <NewUser user={defined(state.user)} onUsernameChanged={handleUsernameChanged} />;
    }
  }

  // Hoist the styled wrapper outside <Switch>: wouter's Switch only
  // descends into Fragments (see flattenChildren in wouter's source),
  // so a <div> child silently disables first-match semantics. Order
  // routes literal-first so any future overlap with the dynamic
  // ":username/:projectName" pattern still resolves to the literal.
  return (
    <div className={styles.inner}>
      <Switch>
        <Route path="/" component={home} />
        <Route path="/new" component={home} />
        <Route path="/:username/:projectName" component={editor} />
      </Switch>
    </div>
  );
}

export function App(): React.JSX.Element {
  return (
    <React.StrictMode>
      <InnerApp />
    </React.StrictMode>
  );
}
