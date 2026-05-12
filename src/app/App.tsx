// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { initializeApp } from '@firebase/app';
import {
  getAuth,
  connectAuthEmulator,
  onAuthStateChanged,
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
      const response = await resultPromise;
      const status = response.status;
      const user = status >= 200 && status < 400 ? await response.json() : undefined;

      return [user, status];
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

// Exported for unit tests; production code only constructs InnerApp via <App>.
// We need the export so tests can drive authStateChanged / asyncAuthStateChanged
// directly without rendering through the firebase/onAuthStateChanged plumbing.
export class InnerApp extends React.PureComponent<{}, AppState> {
  state: AppState;
  // Pending setTimeout(0) handle for the deferred getUserInfo(), and the
  // onAuthStateChanged unsubscribe function. Held so componentWillUnmount can
  // cancel/tear them down. Set up in componentDidMount -- see the comment there.
  private getUserInfoTimer: ReturnType<typeof setTimeout> | null = null;
  private authUnsubscribe: (() => void) | null = null;

  constructor(props: {}) {
    super(props);

    const isDevServer = process.env.NODE_ENV === 'development';
    const auth = getAuth(firebaseApp);
    if (isDevServer) {
      connectAuthEmulator(auth, 'http://localhost:9099', { disableWarnings: true });
    }

    this.state = {
      authUnknown: true,
      auth,
    };
    // The auth-state subscription and the deferred getUserInfo() are wired up
    // in componentDidMount, not here -- see the comment there. Keep this
    // constructor side-effect free (auth object construction is pure setup).
  }

  componentDidMount() {
    // React 18 StrictMode (dev) double-invokes the render phase (so a second
    // InnerApp is constructed and discarded -- it never reaches this method)
    // and, on the committed instance, runs componentDidMount ->
    // componentWillUnmount -> componentDidMount without re-running the
    // constructor. Registering the onAuthStateChanged observer and scheduling
    // getUserInfo() here (and undoing both in componentWillUnmount) keeps the
    // discarded instance from setState()ing on something React never committed
    // and from being pinned alive by the firebase auth event hub, and makes
    // the StrictMode cycle subscribe -> unsubscribe -> subscribe / schedule ->
    // cancel -> schedule rather than leaking the first of each.
    this.authUnsubscribe = onAuthStateChanged(this.state.auth, this.authStateChanged);
    this.getUserInfoTimer = setTimeout(this.getUserInfo);
  }

  componentWillUnmount() {
    if (this.authUnsubscribe) {
      this.authUnsubscribe();
      this.authUnsubscribe = null;
    }
    if (this.getUserInfoTimer !== null) {
      clearTimeout(this.getUserInfoTimer);
      this.getUserInfoTimer = null;
    }
  }

  // Firebase invokes authStateChanged synchronously from its event hub. The
  // previous setTimeout-around-async pattern made this fire-and-forget: any
  // rejection from getIdToken (revoked token, network failure) or maybeLogin
  // (server `/session` error) was silently dropped. Await directly and
  // surface failures via console.error so they're at least visible in dev
  // tools. Returning the promise lets tests `await` the full chain.
  authStateChanged = async (user: FirebaseUser | null): Promise<void> => {
    try {
      await this.asyncAuthStateChanged(user);
    } catch (err) {
      // We deliberately do NOT setState an error here: this method runs on
      // every auth-state transition (including sign-out), and the app's
      // top-level auth gate will re-render Login if needed. Logging keeps
      // the failure visible without overwriting unrelated UI state.
      console.error('auth state change failed:', err);
    }
  };

  asyncAuthStateChanged = async (user: FirebaseUser | null) => {
    if (!user) {
      this.setState({ firebaseIdToken: null });
      return;
    }

    const firebaseIdToken = await user.getIdToken();
    this.setState({ firebaseIdToken });
    await this.maybeLogin(undefined, firebaseIdToken);
  };

  async maybeLogin(authIsKnown = false, firebaseIdToken?: string): Promise<void> {
    authIsKnown = authIsKnown || !this.state.authUnknown;
    if (!authIsKnown) {
      return;
    }

    // if we know the user, we don't need to log in
    const [user] = await userInfo.get();
    if (user) {
      return;
    }

    const idToken = firebaseIdToken ?? this.state.firebaseIdToken;
    if (idToken === null || idToken === undefined) {
      return;
    }

    const bodyContents = {
      idToken,
    };

    const base = this.getBaseURL();
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
      // this.appendModelError(errorMsg);
      console.log(`session error: ${errorMsg}`);
      return undefined;
    }

    this.handleUsernameChanged();
  }

  getUserInfo = async (): Promise<void> => {
    const [user, status] = await userInfo.get();
    if (!(status >= 200 && status < 400) || !user) {
      this.setState({
        authUnknown: false,
      });
      await this.maybeLogin(true);
      return;
    }
    const isNewUser = user.id.startsWith(`temp-`);
    this.setState({
      authUnknown: false,
      isNewUser,
      user,
    });
  };

  handleUsernameChanged = () => {
    setTimeout(async () => {
      await userInfo.invalidate();
      await this.getUserInfo();
    });
  };

  getBaseURL(): string {
    return '';
  }

  editor = (props: RouteComponentProps<EditorMatchParams>) => {
    const { username, projectName } = props.params;
    const user = this.state.user;
    const readOnlyMode = !user || user.id !== username;

    return (
      <HostedWebEditor
        username={username}
        projectName={projectName}
        baseURL={this.getBaseURL()}
        readOnlyMode={readOnlyMode}
      />
    );
  };

  home = (_props: RouteComponentProps) => {
    const location = useLocation()[0];

    const isNewProject = location === '/new';
    return <Home isNewProject={isNewProject} user={defined(this.state.user)} />;
  };

  render() {
    const urlParams = new URLSearchParams(window.location.search);
    const projectParam = urlParams.get('project');
    if (projectParam) return <Redirect to={projectParam} />;

    // if a user is navigating to a project,
    // skip the high level auth check, to enable public models
    if (!/\/.*\/.*/.test(window.location.pathname)) {
      if (!this.state.user) {
        return <Login disabled={this.state.authUnknown} auth={this.state.auth} />;
      }

      if (this.state.isNewUser) {
        return <NewUser user={defined(this.state.user)} onUsernameChanged={this.handleUsernameChanged} />;
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
          <Route path="/" component={this.home} />
          <Route path="/new" component={this.home} />
          <Route path="/:username/:projectName" component={this.editor} />
        </Switch>
      </div>
    );
  }
}

export class App extends React.PureComponent {
  render(): React.JSX.Element {
    return (
      <React.StrictMode>
        <InnerApp />
      </React.StrictMode>
    );
  }
}
