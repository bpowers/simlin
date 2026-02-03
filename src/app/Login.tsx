// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import {
  signInWithRedirect,
  GoogleAuthProvider,
  OAuthProvider,
  Auth as FirebaseAuth,
  fetchSignInMethodsForEmail,
  createUserWithEmailAndPassword,
  updateProfile,
  sendPasswordResetEmail,
  signInWithEmailAndPassword,
} from '@firebase/auth';
import clsx from 'clsx';

import {
  AppleIcon,
  EmailIcon,
  Button,
  SvgIcon,
  Card,
  CardActions,
  CardContent,
  TextLink,
  TextField,
} from '@simlin/diagram';
import { ModelIcon } from '@simlin/diagram/ModelIcon';

import typography from './typography.module.css';

import styles from './Login.module.css';

type EmailLoginStates = 'showEmail' | 'showPassword' | 'showSignup' | 'showProviderRedirect' | 'showRecover';

export interface LoginProps {
  disabled: boolean;
  auth: FirebaseAuth;
}

interface LoginState {
  emailLoginFlow: EmailLoginStates | undefined;
  email: string;
  emailError: string | undefined;
  password: string;
  passwordError: string | undefined;
  fullName: string;
  fullNameError: string | undefined;
  provider: 'google.com' | 'apple.com' | undefined;
}

function appleProvider(): OAuthProvider {
  const provider = new OAuthProvider('apple.com');
  provider.addScope('email');
  provider.addScope('name');
  return provider;
}

export const GoogleIcon: React.FunctionComponent = (props) => {
  return (
    <SvgIcon className={styles.googleIcon} {...props}>
      <path d="M12.545,10.239v3.821h5.445c-0.712,2.315-2.647,3.972-5.445,3.972c-3.332,0-6.033-2.701-6.033-6.032s2.701-6.032,6.033-6.032c1.498,0,2.866,0.549,3.921,1.453l2.814-2.814C17.503,2.988,15.139,2,12.545,2C7.021,2,2.543,6.477,2.543,12s4.478,10,10.002,10c8.396,0,10.249-7.85,9.426-11.748L12.545,10.239z" />
    </SvgIcon>
  );
};

export class Login extends React.Component<LoginProps, LoginState> {
  state: LoginState;

  constructor(props: LoginProps) {
    super(props);

    this.state = {
      emailLoginFlow: undefined,
      email: '',
      emailError: undefined,
      password: '',
      passwordError: undefined,
      fullName: '',
      fullNameError: undefined,
      provider: undefined,
    };
  }

  appleLoginClick = () => {
    const provider = appleProvider();
    setTimeout(async () => {
      await signInWithRedirect(this.props.auth, provider);
    });
  };
  googleLoginClick = () => {
    const provider = new GoogleAuthProvider();
    provider.addScope('profile');
    setTimeout(async () => {
      await signInWithRedirect(this.props.auth, provider);
    });
  };
  emailLoginClick = () => {
    this.setState({ emailLoginFlow: 'showEmail' });
  };
  onFullNameChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    this.setState({ fullName: event.target.value });
  };
  onPasswordChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    this.setState({ password: event.target.value });
  };
  onEmailChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    this.setState({ email: event.target.value });
  };
  onEmailCancel = () => {
    this.setState({ emailLoginFlow: undefined });
  };
  onSubmitEmail = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    const methods = await fetchSignInMethodsForEmail(this.props.auth, email);
    if (methods.includes('password')) {
      this.setState({ emailLoginFlow: 'showPassword' });
    } else if (methods.length === 0) {
      this.setState({ emailLoginFlow: 'showSignup' });
    } else {
      // we only allow 1 method
      const method = methods[0];
      if (method === 'google.com' || method === 'apple.com') {
        this.setState({
          emailLoginFlow: 'showProviderRedirect',
          provider: methods[0] as 'google.com' | 'apple.com',
        });
      } else {
        this.setState({
          emailError: 'an unknown error occurred; try a different email address',
        });
      }
    }
  };
  onSubmitRecovery = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    await sendPasswordResetEmail(this.props.auth, email);

    this.setState({
      emailLoginFlow: 'showPassword',
      password: '',
      passwordError: undefined,
    });
  };
  onSubmitNewUser = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    const fullName = this.state.fullName.trim();
    if (!fullName) {
      this.setState({ fullNameError: 'Enter your name to continue' });
      return;
    }

    const password = this.state.password.trim();
    if (!password) {
      this.setState({ passwordError: 'Enter a password to continue' });
      return;
    }

    try {
      const userCred = await createUserWithEmailAndPassword(this.props.auth, email, password);
      await updateProfile(userCred.user, { displayName: fullName });
    } catch (err) {
      console.log(err);
      if (err instanceof Error) {
        this.setState({ passwordError: err.message });
      } else {
        this.setState({ passwordError: 'something unknown went wrong' });
      }
    }
  };
  onNullSubmit = (event: React.FormEvent<HTMLFormElement>): boolean => {
    event.preventDefault();
    return false;
  };
  onEmailHelp = () => {
    this.setState({ emailLoginFlow: 'showRecover' });
  };
  onEmailLogin = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    const password = this.state.password.trim();
    if (!password) {
      this.setState({ passwordError: 'Enter your email address to continue' });
      return;
    }

    try {
      await signInWithEmailAndPassword(this.props.auth, email, password);
    } catch (err) {
      console.log(err);
      if (err instanceof Error) {
        this.setState({ passwordError: err.message });
      }
    }
  };
  render() {
    const disabledClass = this.props.disabled ? styles.disabled : styles.innerInner;

    let loginUI: React.JSX.Element | undefined = undefined;
    if (!this.props.disabled) {
      switch (this.state.emailLoginFlow) {
        case 'showEmail':
          loginUI = (
            <Card
              variant="outlined"
              style={{ minWidth: 275, maxWidth: 360, width: '100%' }}
              className={styles.emailForm}
            >
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Sign in with email</h6>
                  <TextField
                    label="Email"
                    value={this.state.email}
                    onChange={this.onEmailChanged}
                    type="email"
                    margin="normal"
                    variant="standard"
                    error={this.state.emailError !== undefined}
                    helperText={this.state.emailError}
                    fullWidth
                    autoFocus
                  />
                </CardContent>
                <CardActions>
                  <Button style={{ marginLeft: 'auto' }} onClick={this.onEmailCancel}>
                    Cancel
                  </Button>
                  <Button type="submit" variant="contained" onClick={this.onSubmitEmail}>
                    Next
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        case 'showPassword':
          loginUI = (
            <Card
              variant="outlined"
              style={{ minWidth: 275, maxWidth: 360, width: '100%' }}
              className={styles.emailForm}
            >
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Sign in</h6>
                  <TextField
                    label="Email"
                    value={this.state.email}
                    onChange={this.onEmailChanged}
                    type="email"
                    margin="normal"
                    variant="standard"
                    error={this.state.emailError !== undefined}
                    helperText={this.state.emailError}
                    fullWidth
                  />
                  <TextField
                    label="Password"
                    value={this.state.password}
                    onChange={this.onPasswordChanged}
                    type="password"
                    autoComplete="current-password"
                    margin="normal"
                    variant="standard"
                    error={this.state.passwordError !== undefined}
                    helperText={this.state.passwordError}
                    fullWidth
                    autoFocus
                  />
                </CardContent>
                <CardActions>
                  <span className={typography.body2} style={{ marginRight: 'auto' }}>
                    <TextLink style={{ cursor: 'help' }} underline="hover" onClick={this.onEmailHelp}>
                      Trouble signing in?
                    </TextLink>
                  </span>
                  <Button type="submit" variant="contained" onClick={this.onEmailLogin}>
                    Sign in
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        case 'showSignup':
          loginUI = (
            <Card
              variant="outlined"
              style={{ minWidth: 275, maxWidth: 360, width: '100%' }}
              className={styles.emailForm}
            >
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Create account</h6>
                  <TextField
                    label="Email"
                    value={this.state.email}
                    onChange={this.onEmailChanged}
                    type="email"
                    margin="normal"
                    variant="standard"
                    error={this.state.emailError !== undefined}
                    helperText={this.state.emailError}
                    fullWidth
                  />
                  <TextField
                    label="First & last name"
                    value={this.state.fullName}
                    onChange={this.onFullNameChanged}
                    margin="normal"
                    variant="standard"
                    error={this.state.fullNameError !== undefined}
                    helperText={this.state.fullNameError}
                    fullWidth
                    autoFocus
                  />
                  <TextField
                    label="Choose password"
                    value={this.state.password}
                    onChange={this.onPasswordChanged}
                    type="password"
                    autoComplete="current-password"
                    margin="normal"
                    variant="standard"
                    error={this.state.passwordError !== undefined}
                    helperText={this.state.passwordError}
                    fullWidth
                  />
                </CardContent>
                <CardActions>
                  <Button style={{ marginLeft: 'auto' }} onClick={this.onEmailCancel}>
                    Cancel
                  </Button>
                  <Button type="submit" variant="contained" onClick={this.onSubmitNewUser}>
                    Save
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        case 'showProviderRedirect':
          const provider = this.state.provider === 'google.com' ? 'Google' : 'Apple';
          loginUI = (
            <Card
              variant="outlined"
              style={{ minWidth: 275, maxWidth: 360, width: '100%' }}
              className={styles.emailForm}
            >
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Sign in - you already have an account</h6>
                  <p className={styles.recoverInstructions}>
                    You've already used {provider} to sign up with <b>{this.state.email}</b>. Sign in with {provider} to
                    continue.
                  </p>
                </CardContent>
                <CardActions>
                  <Button
                    style={{ marginLeft: 'auto' }}
                    type="submit"
                    variant="contained"
                    onClick={this.state.provider === 'google.com' ? this.googleLoginClick : this.appleLoginClick}
                  >
                    Sign in with {provider}
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        case 'showRecover':
          loginUI = (
            <Card
              variant="outlined"
              style={{ minWidth: 275, maxWidth: 360, width: '100%' }}
              className={styles.emailForm}
            >
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Recover password</h6>
                  <p className={styles.recoverInstructions}>
                    Get instructions sent to this email that explain how to reset your password
                  </p>
                  <TextField
                    label="Email"
                    value={this.state.email}
                    onChange={this.onEmailChanged}
                    type="email"
                    margin="normal"
                    variant="standard"
                    error={this.state.emailError !== undefined}
                    helperText={this.state.emailError}
                    fullWidth
                    autoFocus
                  />
                </CardContent>
                <CardActions>
                  <Button style={{ marginLeft: 'auto' }} onClick={this.onEmailCancel}>
                    Cancel
                  </Button>
                  <Button type="submit" variant="contained" onClick={this.onSubmitRecovery}>
                    Send
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        default:
          loginUI = (
            <div className={styles.optionsButtons}>
              <Button
                variant="contained"
                style={{ backgroundColor: 'black' }}
                startIcon={<AppleIcon />}
                onClick={this.appleLoginClick}
              >
                Sign in with Apple
              </Button>
              <br />
              <Button variant="contained" color="primary" startIcon={<GoogleIcon />} onClick={this.googleLoginClick}>
                Sign in with Google
              </Button>
              <br />
              <Button
                variant="contained"
                style={{ backgroundColor: '#db4437' }}
                startIcon={<EmailIcon />}
                onClick={this.emailLoginClick}
              >
                Sign in with email
              </Button>
              <br />
            </div>
          );
      }
    }

    return (
      <div className={clsx(styles.outer)}>
        <div className={styles.middle}>
          <div className={styles.inner}>
            <ModelIcon className={styles.logo} />
            <br />
            <div className={disabledClass}>{loginUI}</div>
          </div>
        </div>
      </div>
    );
  }
}
