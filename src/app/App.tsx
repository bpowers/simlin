// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { useLocation, Route, RouteComponentProps, Switch, Redirect } from 'wouter';

import { defined } from '@simlin/core/common';
import { HostedWebEditor } from '@simlin/diagram/HostedWebEditor';

import Home from './Home';
import { Login } from './Login';
import { NewUser } from './NewUser';
import { User } from './User';

import styles from './App.module.css';

interface EditorMatchParams {
  username: string;
  projectName: string;

  readonly [paramName: string | number]: string | undefined;
}

class UserInfoSingleton {
  private resultPromise?: Promise<[User | undefined, number]>;
  private result?: [User | undefined, number];
  constructor() {
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
      await resultPromise;
    }
  }
}

const userInfo = new UserInfoSingleton();

interface AppState {
  authUnknown: boolean;
  isNewUser?: boolean;
  user?: User;
}

class InnerApp extends React.PureComponent<{}, AppState> {
  state: AppState;

  constructor(props: {}) {
    super(props);

    this.state = {
      authUnknown: true,
    };

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

    if (!/\/.*\/.*/.test(window.location.pathname)) {
      if (!this.state.user) {
        return <Login disabled={this.state.authUnknown} onLoginSuccess={this.handleUsernameChanged} />;
      }

      if (this.state.isNewUser) {
        return <NewUser user={defined(this.state.user)} onUsernameChanged={this.handleUsernameChanged} />;
      }
    }

    return (
      <React.Fragment>
        <Switch>
          <div className={styles.inner}>
            <Route path="/" component={this.home} />
            <Route path="/:username/:projectName" component={this.editor} />
            <Route path="/new" component={this.home} />
          </div>
        </Switch>
      </React.Fragment>
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
