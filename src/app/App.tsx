// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import firebase from 'firebase/app';
import 'firebase/auth';
import { BrowserRouter, Route, RouteComponentProps } from 'react-router-dom';

import { createGenerateClassName, StylesProvider } from '@material-ui/styles';

import { createMuiTheme, createStyles, MuiThemeProvider, withStyles, WithStyles } from '@material-ui/core/styles';

import CssBaseline from '@material-ui/core/CssBaseline';

import { defined } from '@system-dynamics/core/common';
import Home from './Home';
import { Login } from './Login';
import { HostedWebEditor } from '@system-dynamics/diagram/HostedWebEditor';
import { NewUser } from './NewUser';
import { User } from './User';

const config = {
  apiKey: 'AIzaSyB9pGSjgGJAOIZPH0HCuofOVDNMiX-XoGI',
  authDomain: 'net-systemdynamics.firebaseapp.com',
};
firebase.initializeApp(config);

const styles = createStyles({
  modelApp: {
    height: '100%',
    width: '100%',
    margin: 0,
    border: 0,
    padding: 0,
  },
});

const generateClassName = createGenerateClassName({
  productionPrefix: 'm',
});

interface EditorMatchParams {
  username: string;
  projectName: string;
}

const theme = createMuiTheme({
  palette: {
    /* primary: purple,
     * secondary: green, */
  },
});

class UserInfoSingleton {
  private resultPromise?: Promise<Response>;
  private result?: [User | undefined, number];
  constructor() {
    // store this promise; we might race calling get() below, but all racers will
    // await this single fetch result.
    this.fetch();
  }

  private fetch(): void {
    this.resultPromise = fetch('/api/user', { credentials: 'same-origin' });
  }

  async get(): Promise<[User | undefined, number]> {
    if (this.resultPromise) {
      const response = await this.resultPromise;
      const status = response.status;
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const user = status >= 200 && status < 400 ? await response.json() : undefined;

      this.result = [user, status];
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
  auth: firebase.auth.Auth;
  firebaseIdToken?: string | null;
}
type AppProps = WithStyles<typeof styles>;

const InnerApp = withStyles(styles)(
  class InnerApp extends React.PureComponent<AppProps, AppState> {
    state: AppState;

    constructor(props: AppProps) {
      super(props);

      const isDevServer = process.env.NODE_ENV === 'development';
      const auth = firebase.auth();
      if (isDevServer) {
        // eslint-disable-next-line @typescript-eslint/ban-ts-comment
        // @ts-ignore
        auth.useEmulator('http://localhost:9099', { disableWarnings: true });
      }

      this.state = {
        authUnknown: true,
        auth,
      };

      // notify our app when a user logs in
      firebase.auth().onAuthStateChanged(this.authStateChanged);

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.getUserInfo);
    }

    authStateChanged = (user: firebase.User | null) => {
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.asyncAuthStateChanged, undefined, user);
    };

    asyncAuthStateChanged = async (user: firebase.User | null) => {
      if (!user) {
        this.setState({ firebaseIdToken: null });
        return;
      }

      const firebaseIdToken = await user.getIdToken();
      this.setState({ firebaseIdToken });
      await this.maybeLogin(undefined, firebaseIdToken);
    };

    async maybeLogin(authIsKnown = false, firebaseIdToken?: string): Promise<void> {
      if (!(authIsKnown || !this.state.authUnknown)) {
        console.log(`maybeLogin exit auth`);
        return;
      }

      const idToken = firebaseIdToken ?? this.state.firebaseIdToken;
      if (idToken === null || idToken === undefined) {
        console.log(`maybeLogin exit no id`);
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
      const { username, projectName } = props.match.params;
      return (
        <HostedWebEditor
          username={username}
          projectName={projectName}
          baseURL={this.getBaseURL()}
          history={props.history}
        />
      );
    };

    home = (props: RouteComponentProps) => {
      const isNewProject = props.location.pathname === '/new';
      return <Home isNewProject={isNewProject} user={defined(this.state.user)} />;
    };

    render() {
      if (!this.state.user) {
        return <Login disabled={this.state.authUnknown} auth={this.state.auth} />;
      }

      if (this.state.isNewUser) {
        return <NewUser user={defined(this.state.user)} onUsernameChanged={this.handleUsernameChanged} />;
      }

      const { classes } = this.props;

      return (
        <BrowserRouter>
          <div className={classes.modelApp}>
            <Route exact path="/" component={this.home} />
            <Route exact path="/:username/:projectName" render={this.editor} />
            <Route exact path="/new" component={this.home} />
          </div>
        </BrowserRouter>
      );
    }
  },
);

export class App extends React.PureComponent {
  render(): JSX.Element {
    return (
      <StylesProvider generateClassName={generateClassName}>
        <MuiThemeProvider theme={theme}>
          <CssBaseline />
          <InnerApp />
        </MuiThemeProvider>
      </StylesProvider>
    );
  }
}
