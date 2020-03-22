// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { BrowserRouter, Route, RouteComponentProps } from 'react-router-dom';

import { createGenerateClassName, StylesProvider } from '@material-ui/styles';

import { createMuiTheme, createStyles, MuiThemeProvider, withStyles, WithStyles } from '@material-ui/core/styles';

import CssBaseline from '@material-ui/core/CssBaseline';

import { defined } from './common';
import Home from './Home';
import { Login } from './Login';
import { Editor } from './model/Editor';
import { NewUser } from './NewUser';
import { User } from './User';

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
    // await thins single fetch result.
    this.fetch();
  }

  private fetch(): void {
    this.resultPromise = fetch('/api/user', { credentials: 'same-origin' });
  }

  async get(): Promise<[User | undefined, number]> {
    if (this.resultPromise) {
      const response = await this.resultPromise;
      const status = response.status;
      const user = status >= 200 && status < 400 ? await response.json() : undefined;

      this.result = [user, status];
      this.resultPromise = undefined;
    }

    return defined(this.result);
  }

  async invalidate(): Promise<void> {
    if (this.resultPromise) {
      await this.resultPromise;
      this.resultPromise = undefined;
    }

    this.result = undefined;
    this.fetch();
  }
}

const userInfo = new UserInfoSingleton();

interface AppState {
  authUnknown: boolean;
  isNewUser?: boolean;
  user?: User;
}
type AppProps = WithStyles<typeof styles>;

const InnerApp = withStyles(styles)(
  class InnerApp extends React.PureComponent<AppProps, AppState> {
    state: AppState;

    constructor(props: AppProps) {
      super(props);

      this.state = {
        authUnknown: true,
      };

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.getUserInfo);
    }

    getUserInfo = async (): Promise<void> => {
      const [user, status] = await userInfo.get();
      if (!(status >= 200 && status < 400) || !user) {
        this.setState({
          authUnknown: false,
        });
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

    editor = (props: RouteComponentProps<EditorMatchParams>) => {
      const { username, projectName } = props.match.params;
      return <Editor username={username} projectName={projectName} baseURL="" history={props.history} />;
    };

    home = (props: RouteComponentProps<{}>) => {
      const isNewProject = props.location.pathname === '/new';
      return <Home isNewProject={isNewProject} user={defined(this.state.user)} />;
    };

    render() {
      if (!this.state.user) {
        return <Login disabled={this.state.authUnknown} />;
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
  render() {
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
