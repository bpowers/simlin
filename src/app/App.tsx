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

interface AppState {
  authUnknown: boolean;
  isNewUser?: boolean;
  user?: User;
}
interface AppProps extends WithStyles<typeof styles> {}

const InnerApp = withStyles(styles)(
  class extends React.PureComponent<AppProps, AppState> {
    state: AppState;

    constructor(props: AppProps) {
      super(props);

      this.state = {
        authUnknown: true,
      };

      setTimeout(this.getUserInfo);
    }

    getUserInfo = async (): Promise<void> => {
      const response = await fetch('/api/user', { credentials: 'same-origin' });
      const status = response.status;
      if (!(status >= 200 && status < 400)) {
        this.setState(prevState => ({
          authUnknown: false,
        }));
        return;
      }
      const user: User = await response.json();
      const isNewUser = user.id.startsWith(`temp-`);
      this.setState(prevState => ({
        authUnknown: false,
        isNewUser,
        user,
      }));
    };

    handleUsernameChanged = () => {
      setTimeout(this.getUserInfo);
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
