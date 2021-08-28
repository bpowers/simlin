// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import StyledFirebaseAuth from 'react-firebaseui/StyledFirebaseAuth';
import firebase from 'firebase/app';
import 'firebase/auth';
import clsx from 'clsx';
import { styled } from '@material-ui/core/styles';

import { ModelIcon } from '@system-dynamics/diagram/ModelIcon';

export interface LoginProps {
  disabled: boolean;
  auth: firebase.auth.Auth;
}

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

export const Login = styled(
  class Login extends React.Component<LoginProps & { className?: string }> {
    render() {
      const { className } = this.props;
      const disabledClass = this.props.disabled ? 'simlin-login-disabled' : '';

      const loginUI = !this.props.disabled ? (
        <StyledFirebaseAuth uiConfig={uiConfig} firebaseAuth={this.props.auth} />
      ) : undefined;

      return (
        <div className={clsx(className, 'simlin-login-outer')}>
          <div className="simlin-login-middle">
            <div className="simlin-login-inner">
              <ModelIcon className="simlin-login-logo" />
              <br />
              <div className={disabledClass}>{loginUI}</div>
            </div>
          </div>
        </div>
      );
    }
  },
)(() => ({
  '&.simlin-login-outer': {
    display: 'table',
    position: 'absolute',
    height: '100%',
    width: '100%',
  },
  '.simlin-login-middle': {
    display: 'table-cell',
    verticalAlign: 'middle',
  },
  '.simlin-login-inner': {
    marginLeft: 'auto',
    marginRight: 'auto',
    textAlign: 'center',
  },
  '.simlin-login-disabled': {
    pointerEvents: 'none',
    opacity: 0,
  },
  '.simlin-login-logo': {
    width: 160,
    height: 160,
  },
}));
