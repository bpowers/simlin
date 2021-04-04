// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import StyledFirebaseAuth from 'react-firebaseui/StyledFirebaseAuth';
import firebase from 'firebase/app';
import 'firebase/auth';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { ModelIcon } from '@system-dynamics/diagram/ModelIcon';

const styles = createStyles({
  loginOuter: {
    display: 'table',
    position: 'absolute',
    height: '100%',
    width: '100%',
  },
  loginMiddle: {
    display: 'table-cell',
    verticalAlign: 'middle',
  },
  loginInner: {
    marginLeft: 'auto',
    marginRight: 'auto',
    textAlign: 'center',
  },
  loginDisabled: {
    pointerEvents: 'none',
    opacity: 0,
  },
  logo: {
    width: 160,
    height: 160,
  },
});

interface LoginPropsFull extends WithStyles<typeof styles> {
  disabled: boolean;
  auth: firebase.auth.Auth;
}

export type LoginProps = Pick<LoginPropsFull, 'disabled' | 'auth'>;

function appleProvider(): string {
  const provider = new firebase.auth.OAuthProvider('apple.com');
  provider.addScope('email');
  provider.addScope('name');
  return provider.providerId;
}

const uiConfig = {
  signInFlow: 'redirect',
  signInSuccessUrl: '/',
  signInOptions: [
    appleProvider(),
    firebase.auth.GoogleAuthProvider.PROVIDER_ID,
    firebase.auth.EmailAuthProvider.PROVIDER_ID,
  ],
};

export const Login = withStyles(styles)(
  class Login extends React.Component<LoginPropsFull> {
    render() {
      const { classes } = this.props;
      const disabledClass = this.props.disabled ? classes.loginDisabled : '';

      const loginUI = !this.props.disabled ? (
        <StyledFirebaseAuth uiConfig={uiConfig} firebaseAuth={this.props.auth} />
      ) : undefined;

      return (
        <div className={classes.loginOuter}>
          <div className={classes.loginMiddle}>
            <div className={classes.loginInner}>
              <ModelIcon className={classes.logo} />
              <br />
              <div className={disabledClass}>{loginUI}</div>
            </div>
          </div>
        </div>
      );
    }
  },
);
