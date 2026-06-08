// Copyright 2026 The Simlin Authors. All rights reserved.
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

const initialState: LoginState = {
  emailLoginFlow: undefined,
  email: '',
  emailError: undefined,
  password: '',
  passwordError: undefined,
  fullName: '',
  fullNameError: undefined,
  provider: undefined,
};

export function Login(props: LoginProps): React.JSX.Element {
  // One useState object with a class-parity merging `setState` helper preserves
  // the class's behavior for handlers that issue several setState calls in one
  // turn (each merges a partial patch onto the previous state).
  const [state, setStateRaw] = React.useState<LoginState>(() => ({ ...initialState }));

  const setState = React.useCallback((patch: Partial<LoginState>): void => {
    setStateRaw((prev) => ({ ...prev, ...patch }));
  }, []);

  // The async auth handlers escape the render that created them (they await
  // firebase calls), so they read the freshest props/state through this ref:
  // a prop change (auth) or an interleaved state edit between handler creation
  // and the await's resolution is observed correctly, matching the class's
  // this.props / this.state reads.
  const latest = React.useRef<{ props: LoginProps; state: LoginState }>(
    undefined as unknown as { props: LoginProps; state: LoginState },
  );
  latest.current = { props, state };

  // Surface OAuth redirect failures (provider misconfig, popup blocked,
  // network errors, expired auth domain) into emailError so the user sees
  // them. The previous setTimeout-around-async pattern silently swallowed
  // rejections because nothing awaited or .catch'd the inner promise.
  const appleLoginClick = async () => {
    const provider = appleProvider();
    try {
      await signInWithRedirect(latest.current.props.auth, provider);
    } catch (err) {
      setState({
        emailError: err instanceof Error ? err.message : 'Sign in with Apple failed',
      });
    }
  };
  const googleLoginClick = async () => {
    const provider = new GoogleAuthProvider();
    provider.addScope('profile');
    try {
      await signInWithRedirect(latest.current.props.auth, provider);
    } catch (err) {
      setState({
        emailError: err instanceof Error ? err.message : 'Sign in with Google failed',
      });
    }
  };
  const emailLoginClick = () => {
    setState({ emailLoginFlow: 'showEmail' });
  };
  // Editing a field clears its stale error: once the user starts correcting
  // the value, keeping the old red message would be misleading (it described
  // the previous submission, not the text on screen).
  const onFullNameChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ fullName: event.target.value, fullNameError: undefined });
  };
  const onPasswordChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ password: event.target.value, passwordError: undefined });
  };
  const onEmailChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ email: event.target.value, emailError: undefined });
  };
  const onEmailCancel = () => {
    setState({ emailLoginFlow: undefined });
  };
  const onSubmitEmail = async () => {
    const email = latest.current.state.email.trim();
    if (!email) {
      setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    // fetchSignInMethodsForEmail rejects on network errors, rate limiting,
    // and Firebase's email-enumeration protection; the sibling auth handlers
    // all surface failures, so this one must too (it used to escape as an
    // unhandled rejection and the form just sat there).
    let methods: string[];
    try {
      methods = await fetchSignInMethodsForEmail(latest.current.props.auth, email);
    } catch (err) {
      setState({
        emailError: err instanceof Error ? err.message : 'unable to look up that email address; try again',
      });
      return;
    }
    if (methods.includes('password')) {
      setState({ emailLoginFlow: 'showPassword' });
    } else if (methods.length === 0) {
      setState({ emailLoginFlow: 'showSignup' });
    } else {
      // we only allow 1 method
      const method = methods[0];
      if (method === 'google.com' || method === 'apple.com') {
        setState({
          emailLoginFlow: 'showProviderRedirect',
          provider: methods[0] as 'google.com' | 'apple.com',
        });
      } else {
        setState({
          emailError: 'an unknown error occurred; try a different email address',
        });
      }
    }
  };
  const onSubmitRecovery = async () => {
    const email = latest.current.state.email.trim();
    if (!email) {
      setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    try {
      await sendPasswordResetEmail(latest.current.props.auth, email);
    } catch (err) {
      // Stay on the recovery card with a visible error rather than advancing
      // as if the reset email had been sent.
      setState({
        emailError: err instanceof Error ? err.message : 'sending the recovery email failed; try again',
      });
      return;
    }

    setState({
      emailLoginFlow: 'showPassword',
      password: '',
      passwordError: undefined,
    });
  };
  const onSubmitNewUser = async () => {
    const email = latest.current.state.email.trim();
    if (!email) {
      setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    const fullName = latest.current.state.fullName.trim();
    if (!fullName) {
      setState({ fullNameError: 'Enter your name to continue' });
      return;
    }

    const password = latest.current.state.password.trim();
    if (!password) {
      setState({ passwordError: 'Enter a password to continue' });
      return;
    }

    try {
      const userCred = await createUserWithEmailAndPassword(latest.current.props.auth, email, password);
      await updateProfile(userCred.user, { displayName: fullName });
    } catch (err) {
      console.log(err);
      if (err instanceof Error) {
        setState({ passwordError: err.message });
      } else {
        setState({ passwordError: 'something unknown went wrong' });
      }
    }
  };
  const onNullSubmit = (event: React.FormEvent<HTMLFormElement>): boolean => {
    event.preventDefault();
    return false;
  };
  const onEmailHelp = () => {
    setState({ emailLoginFlow: 'showRecover' });
  };
  const onEmailLogin = async () => {
    const email = latest.current.state.email.trim();
    if (!email) {
      setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    const password = latest.current.state.password.trim();
    if (!password) {
      setState({ passwordError: 'Enter your email address to continue' });
      return;
    }

    try {
      await signInWithEmailAndPassword(latest.current.props.auth, email, password);
    } catch (err) {
      console.log(err);
      if (err instanceof Error) {
        setState({ passwordError: err.message });
      }
    }
  };

  const disabledClass = props.disabled ? styles.disabled : styles.innerInner;

  let loginUI: React.JSX.Element | undefined = undefined;
  if (!props.disabled) {
    switch (state.emailLoginFlow) {
      case 'showEmail':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Sign in with email</h6>
                <TextField
                  label="Email"
                  value={state.email}
                  onChange={onEmailChanged}
                  type="email"
                  margin="normal"
                  variant="standard"
                  error={state.emailError !== undefined}
                  helperText={state.emailError}
                  fullWidth
                  autoFocus
                />
              </CardContent>
              <CardActions>
                <Button style={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
                  Cancel
                </Button>
                <Button type="submit" variant="contained" onClick={onSubmitEmail}>
                  Next
                </Button>
              </CardActions>
            </form>
          </Card>
        );
        break;
      case 'showPassword':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Sign in</h6>
                <TextField
                  label="Email"
                  value={state.email}
                  onChange={onEmailChanged}
                  type="email"
                  margin="normal"
                  variant="standard"
                  error={state.emailError !== undefined}
                  helperText={state.emailError}
                  fullWidth
                />
                <TextField
                  label="Password"
                  value={state.password}
                  onChange={onPasswordChanged}
                  type="password"
                  autoComplete="current-password"
                  margin="normal"
                  variant="standard"
                  error={state.passwordError !== undefined}
                  helperText={state.passwordError}
                  fullWidth
                  autoFocus
                />
              </CardContent>
              <CardActions>
                <span className={typography.body2} style={{ marginRight: 'auto' }}>
                  <TextLink style={{ cursor: 'help' }} underline="hover" onClick={onEmailHelp}>
                    Trouble signing in?
                  </TextLink>
                </span>
                <Button type="submit" variant="contained" onClick={onEmailLogin}>
                  Sign in
                </Button>
              </CardActions>
            </form>
          </Card>
        );
        break;
      case 'showSignup':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Create account</h6>
                <TextField
                  label="Email"
                  value={state.email}
                  onChange={onEmailChanged}
                  type="email"
                  margin="normal"
                  variant="standard"
                  error={state.emailError !== undefined}
                  helperText={state.emailError}
                  fullWidth
                />
                <TextField
                  label="First & last name"
                  value={state.fullName}
                  onChange={onFullNameChanged}
                  margin="normal"
                  variant="standard"
                  error={state.fullNameError !== undefined}
                  helperText={state.fullNameError}
                  fullWidth
                  autoFocus
                />
                <TextField
                  label="Choose password"
                  value={state.password}
                  onChange={onPasswordChanged}
                  type="password"
                  autoComplete="current-password"
                  margin="normal"
                  variant="standard"
                  error={state.passwordError !== undefined}
                  helperText={state.passwordError}
                  fullWidth
                />
              </CardContent>
              <CardActions>
                <Button style={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
                  Cancel
                </Button>
                <Button type="submit" variant="contained" onClick={onSubmitNewUser}>
                  Save
                </Button>
              </CardActions>
            </form>
          </Card>
        );
        break;
      case 'showProviderRedirect': {
        const provider = state.provider === 'google.com' ? 'Google' : 'Apple';
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Sign in - you already have an account</h6>
                <p className={styles.recoverInstructions}>
                  You've already used {provider} to sign up with <b>{state.email}</b>. Sign in with {provider} to
                  continue.
                </p>
                {/* The "Sign in with {provider}" button calls googleLoginClick /
                 * appleLoginClick, whose signInWithRedirect rejection is surfaced
                 * via emailError -- render it here so the failure is visible (this
                 * card has no helperText-bearing TextField like the other flows). */}
                {state.emailError !== undefined && (
                  <p role="alert" className={typography.body2}>
                    {state.emailError}
                  </p>
                )}
              </CardContent>
              <CardActions>
                <Button
                  style={{ marginLeft: 'auto' }}
                  type="submit"
                  variant="contained"
                  onClick={state.provider === 'google.com' ? googleLoginClick : appleLoginClick}
                >
                  Sign in with {provider}
                </Button>
              </CardActions>
            </form>
          </Card>
        );
        break;
      }
      case 'showRecover':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Recover password</h6>
                <p className={styles.recoverInstructions}>
                  Get instructions sent to this email that explain how to reset your password
                </p>
                <TextField
                  label="Email"
                  value={state.email}
                  onChange={onEmailChanged}
                  type="email"
                  margin="normal"
                  variant="standard"
                  error={state.emailError !== undefined}
                  helperText={state.emailError}
                  fullWidth
                  autoFocus
                />
              </CardContent>
              <CardActions>
                <Button style={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
                  Cancel
                </Button>
                <Button type="submit" variant="contained" onClick={onSubmitRecovery}>
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
              className={styles.appleButton}
              startIcon={<AppleIcon />}
              onClick={appleLoginClick}
            >
              Sign in with Apple
            </Button>
            <Button variant="contained" color="primary" startIcon={<GoogleIcon />} onClick={googleLoginClick}>
              Sign in with Google
            </Button>
            <Button
              variant="contained"
              className={styles.emailButton}
              startIcon={<EmailIcon />}
              onClick={emailLoginClick}
            >
              Sign in with email
            </Button>
            {/* Visible error sink for OAuth click handlers. Without this, a
             * rejected signInWithRedirect (popup blocked, provider misconfig,
             * network failure) would set emailError but stay invisible until
             * the user enters the email-flow path. */}
            {state.emailError !== undefined && (
              <p role="alert" className={typography.body2}>
                {state.emailError}
              </p>
            )}
          </div>
        );
    }
  }

  return (
    <div className={styles.outer}>
      <div className={styles.middle}>
        <div className={styles.inner}>
          <ModelIcon className={styles.logo} />
          <div className={disabledClass}>{loginUI}</div>
        </div>
      </div>
    </div>
  );
}
