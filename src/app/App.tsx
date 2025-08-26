// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { initializeApp } from 'firebase/app';
import {
  getAuth,
  connectAuthEmulator,
  onAuthStateChanged,
  Auth as FirebaseAuth,
  User as FirebaseUser,
} from 'firebase/auth';

import { useLocation, Route, RouteComponentProps, Switch } from 'wouter';
import { styled } from '@mui/material/styles';
import { createTheme, ThemeProvider } from '@mui/material/styles';
import CssBaseline from '@mui/material/CssBaseline';

import { defined } from '@system-dynamics/core/common';
import { HostedWebEditor } from '@system-dynamics/diagram/HostedWebEditor';

import Home from './Home';
import { Login } from './Login';
import { NewUser } from './NewUser';
import { User } from './User';

// Only import VisualTestPage in development/test environments
const VisualTestPage =
  process.env.NODE_ENV !== 'production'
    ? React.lazy(() => import('./VisualTestPage').then((m) => ({ default: m.VisualTestPage })))
    : null;

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

const theme = createTheme({
  palette: {
    /* primary: purple,
     * secondary: green, */
  },
});

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
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
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
interface AppProps {
  className?: string;
}

const InnerApp = styled(
  class InnerApp extends React.PureComponent<AppProps, AppState> {
    state: AppState;

    constructor(props: AppProps) {
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

      // notify our app when a user logs in
      onAuthStateChanged(auth, this.authStateChanged);

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.getUserInfo);
    }

    authStateChanged = (user: FirebaseUser | null) => {
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.asyncAuthStateChanged, undefined, user);
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
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
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
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
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

    visualTest = (_props: RouteComponentProps) => {
      if (VisualTestPage) {
        return (
          <React.Suspense fallback={<div>Loading...</div>}>
            <VisualTestPage />
          </React.Suspense>
        );
      }
      return <div>Not available in production</div>;
    };

    render() {
      const { className } = this.props;

      // Allow visual test page to bypass authentication (only in development)
      const currentPath = window.location.pathname;
      if (currentPath === '/visual-test' && process.env.NODE_ENV !== 'production' && VisualTestPage) {
        return (
          <React.Fragment>
            <CssBaseline />
            <div className={className}>
              <React.Suspense fallback={<div>Loading test page...</div>}>
                <VisualTestPage />
              </React.Suspense>
            </div>
          </React.Fragment>
        );
      }

      if (!/\/.*\/.*/.test(window.location.pathname)) {
        if (!this.state.user) {
          return <Login disabled={this.state.authUnknown} auth={this.state.auth} />;
        }

        if (this.state.isNewUser) {
          return <NewUser user={defined(this.state.user)} onUsernameChanged={this.handleUsernameChanged} />;
        }
      }

      return (
        <React.Fragment>
          <CssBaseline />
          <Switch>
            <div className={className}>
              <Route path="/" component={this.home} />
              {process.env.NODE_ENV !== 'production' && <Route path="/visual-test" component={this.visualTest} />}
              <Route path="/:username/:projectName" component={this.editor} />
              <Route path="/new" component={this.home} />
            </div>
          </Switch>
        </React.Fragment>
      );
    }
  },
)(`
    height: 100%;
    width: 100%;
    margin: 0px;
    border: 0px;
    padding: 0px;
`);

export class App extends React.PureComponent {
  render(): JSX.Element {
    return (
      <React.StrictMode>
        <ThemeProvider theme={theme}>
          <InnerApp />
        </ThemeProvider>
      </React.StrictMode>
    );
  }
}
