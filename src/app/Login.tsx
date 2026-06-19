// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import {
  signInWithRedirect,
  GoogleAuthProvider,
  OAuthProvider,
  Auth as FirebaseAuth,
  createUserWithEmailAndPassword,
  updateProfile,
  sendPasswordResetEmail,
  signInWithEmailAndPassword,
} from '@firebase/auth';
import {
  AppleIcon,
  EmailIcon,
  Button,
  CircularProgress,
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

// The email flow is a single combined sign-in card rather than a method-prefetch
// router. We deliberately do NOT call Firebase's deprecated
// fetchSignInMethodsForEmail: it enables email enumeration, and when a project
// turns on Email Enumeration Protection it returns [] for every address, which
// the old flow misread as "no account" and used to route every existing
// password user to the signup card -- a silent, total lockout (issue #692).
// Removing the prefetch makes the flow correct whether that protection is on or
// off, at the cost of no longer being able to tell a returning OAuth-only user
// which provider they used: a wrong password and an OAuth-only account both
// surface as auth/invalid-credential under protection, so we show one generic
// message and point such users back to the provider buttons on the landing
// screen.
type EmailLoginStates = 'signin' | 'signup' | 'recover' | 'recoverSent';

export interface LoginProps {
  disabled: boolean;
  auth: FirebaseAuth;
  // Set by App when the server /session exchange fails after a successful IdP
  // sign-in; shown on the landing screen so the bounce-back isn't silent.
  error?: string;
}

interface LoginState {
  emailLoginFlow: EmailLoginStates | undefined;
  email: string;
  emailError: string | undefined;
  password: string;
  passwordError: string | undefined;
  fullName: string;
  fullNameError: string | undefined;
  // Card-level credential failure on the combined sign-in card (kept separate
  // from the per-field emailError/passwordError so it can be announced via
  // role=alert and cleared when the user edits either field).
  signinError: string | undefined;
}

function appleProvider(): OAuthProvider {
  const provider = new OAuthProvider('apple.com');
  provider.addScope('email');
  provider.addScope('name');
  return provider;
}

/**
 * Firebase Auth errors carry a string `code` (e.g. 'auth/invalid-credential')
 * on top of the Error shape. Read it defensively: a non-Firebase throw (a bare
 * network TypeError, say) has no `code` and must fall through to the generic
 * branch rather than crash the handler.
 */
function firebaseErrorCode(err: unknown): string | undefined {
  if (typeof err === 'object' && err !== null) {
    const code = (err as { code?: unknown }).code;
    if (typeof code === 'string') {
      return code;
    }
  }
  return undefined;
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
  signinError: undefined,
};

export function Login(props: LoginProps): React.JSX.Element {
  // One useState object with a class-parity merging `setState` helper preserves
  // the behavior for handlers that issue several setState calls in one turn
  // (each merges a partial patch onto the previous state).
  const [state, setStateRaw] = React.useState<LoginState>(() => ({ ...initialState }));

  const setState = React.useCallback((patch: Partial<LoginState>): void => {
    setStateRaw((prev) => ({ ...prev, ...patch }));
  }, []);

  // The async auth handlers escape the render that created them (they await
  // firebase calls), so they read the freshest props/state through this ref:
  // a prop change (auth) or an interleaved state edit between handler creation
  // and the await's resolution is observed correctly.
  const latest = React.useRef<{ props: LoginProps; state: LoginState }>(
    undefined as unknown as { props: LoginProps; state: LoginState },
  );
  latest.current = { props, state };

  // Surface OAuth redirect failures (provider misconfig, popup blocked,
  // network errors, expired auth domain) into emailError so the user sees them.
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
  // Entering the email flow lands directly on the combined sign-in card. Clear
  // any stale OAuth error from the landing screen so it doesn't reappear under
  // the email field.
  const emailLoginClick = () => {
    setState({ emailLoginFlow: 'signin', emailError: undefined, passwordError: undefined, signinError: undefined });
  };
  // Editing a field clears its stale error: once the user starts correcting the
  // value, keeping the old red message would be misleading. The combined
  // credential error (signinError) is about "email OR password", so editing
  // either field clears it.
  const onFullNameChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ fullName: event.target.value, fullNameError: undefined });
  };
  const onPasswordChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ password: event.target.value, passwordError: undefined, signinError: undefined });
  };
  const onEmailChanged = (event: React.ChangeEvent<HTMLInputElement>) => {
    setState({ email: event.target.value, emailError: undefined, signinError: undefined });
  };
  const onCancel = () => {
    setState({
      emailLoginFlow: undefined,
      emailError: undefined,
      passwordError: undefined,
      fullNameError: undefined,
      signinError: undefined,
    });
  };
  const onTroubleSigningIn = () => {
    setState({ emailLoginFlow: 'recover', emailError: undefined, passwordError: undefined, signinError: undefined });
  };
  const onCreateAccount = () => {
    setState({
      emailLoginFlow: 'signup',
      emailError: undefined,
      passwordError: undefined,
      fullNameError: undefined,
      signinError: undefined,
    });
  };
  const onSignInInstead = () => {
    setState({
      emailLoginFlow: 'signin',
      emailError: undefined,
      passwordError: undefined,
      fullNameError: undefined,
      signinError: undefined,
    });
  };
  // Combined sign-in: attempt the password sign-in directly and branch on the
  // Firebase error code. On success App's onAuthStateChanged observer takes
  // over (exchanges the id token for a session), so there's nothing to do here.
  const onSignIn = async () => {
    const email = latest.current.state.email.trim();
    if (!email) {
      setState({ emailError: 'Enter your email address to continue' });
      return;
    }
    const password = latest.current.state.password.trim();
    if (!password) {
      setState({ passwordError: 'Enter your password to continue' });
      return;
    }

    try {
      await signInWithEmailAndPassword(latest.current.props.auth, email, password);
    } catch (err) {
      const code = firebaseErrorCode(err);
      if (code === 'auth/invalid-email') {
        setState({ signinError: "That doesn't look like a valid email address." });
      } else if (code === 'auth/network-request-failed' || code === 'auth/too-many-requests') {
        // Transient, account-agnostic failures: Firebase returns these whether
        // or not the account exists, so reporting them honestly leaks nothing --
        // and it avoids telling a user with correct credentials that their
        // password is wrong during a network outage or rate-limit.
        setState({ signinError: "We couldn't sign you in right now. Please try again in a moment." });
      } else {
        // auth/invalid-credential (the unified wrong-password / no-such-user /
        // OAuth-only error under Email Enumeration Protection),
        // auth/wrong-password, auth/user-not-found, and any non-Firebase throw
        // all collapse to one enumeration-safe message.
        setState({
          signinError:
            'Incorrect email or password. If you used Google or Apple to sign up, go back and use those buttons.',
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
      // Stay on the recovery card with a visible error rather than advancing as
      // if the reset email had been sent.
      setState({
        emailError: err instanceof Error ? err.message : 'sending the recovery email failed; try again',
      });
      return;
    }

    // Neutral confirmation only: sendPasswordResetEmail succeeds even for
    // addresses with no account when Email Enumeration Protection is on, so we
    // must not assert an email was actually sent to a real account.
    setState({ emailLoginFlow: 'recoverSent' });
  };
  const onRecoverDone = () => {
    setState({
      emailLoginFlow: 'signin',
      password: '',
      passwordError: undefined,
      emailError: undefined,
      fullNameError: undefined,
      signinError: undefined,
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
      const code = firebaseErrorCode(err);
      if (code === 'auth/email-already-in-use') {
        setState({ emailError: 'An account with this email already exists.' });
      } else if (code === 'auth/weak-password') {
        setState({ passwordError: 'Choose a password with at least 6 characters.' });
      } else if (code === 'auth/invalid-email') {
        setState({ emailError: "That doesn't look like a valid email address." });
      } else {
        setState({ passwordError: err instanceof Error ? err.message : 'something unknown went wrong' });
      }
    }
  };
  const onNullSubmit = (event: React.FormEvent<HTMLFormElement>): boolean => {
    event.preventDefault();
    return false;
  };

  let loginUI: React.JSX.Element | undefined = undefined;
  if (!props.disabled) {
    switch (state.emailLoginFlow) {
      case 'signin':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Sign in to Simlin</h6>
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
                  // Focus the empty field: email on first entry, password when
                  // an email was carried in from another card.
                  autoFocus={state.email === ''}
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
                  autoFocus={state.email !== ''}
                />
                {state.signinError !== undefined && (
                  <p role="alert" className={clsx(typography.body2, styles.formError)}>
                    {state.signinError}
                  </p>
                )}
                <p className={typography.body2}>
                  <TextLink underline="hover" onClick={onTroubleSigningIn}>
                    Trouble signing in?
                  </TextLink>
                  {' '}
                  <TextLink underline="hover" onClick={onCreateAccount}>
                    Create account
                  </TextLink>
                </p>
              </CardContent>
              <CardActions>
                <Button style={{ marginLeft: 'auto' }} onClick={onCancel}>
                  Cancel
                </Button>
                <Button type="submit" variant="contained" onClick={onSignIn}>
                  Sign in
                </Button>
              </CardActions>
            </form>
          </Card>
        );
        break;
      case 'signup':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <form onSubmit={onNullSubmit}>
              <CardContent>
                <h6 className={typography.heading6}>Create your account</h6>
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
                  autoComplete="new-password"
                  margin="normal"
                  variant="standard"
                  error={state.passwordError !== undefined}
                  helperText={state.passwordError}
                  fullWidth
                />
                <p className={typography.body2}>
                  <TextLink underline="hover" onClick={onSignInInstead}>
                    Already have an account? Sign in instead
                  </TextLink>
                </p>
              </CardContent>
              <CardActions>
                <Button style={{ marginLeft: 'auto' }} onClick={onCancel}>
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
      case 'recover':
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
                <Button style={{ marginLeft: 'auto' }} onClick={onCancel}>
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
      case 'recoverSent':
        loginUI = (
          <Card variant="outlined" className={styles.emailForm}>
            <CardContent>
              <h6 className={typography.heading6}>Check your email</h6>
              <p className={styles.recoverInstructions}>
                If an account exists for <b>{state.email}</b>, we&apos;ve sent password-reset instructions. Follow the
                link in that email to choose a new password.
              </p>
            </CardContent>
            <CardActions>
              <Button style={{ marginLeft: 'auto' }} variant="contained" onClick={onRecoverDone}>
                Done
              </Button>
            </CardActions>
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
             * network failure) would set emailError but stay invisible because
             * no email-flow card is mounted to render it. */}
            {state.emailError !== undefined && (
              <p role="alert" className={clsx(typography.body2, styles.formError)}>
                {state.emailError}
              </p>
            )}
            {/* Server-session failure relayed from App (IdP sign-in succeeded
             * but creating the server session failed, bouncing the user back). */}
            {props.error !== undefined && (
              <p role="alert" className={clsx(typography.body2, styles.formError)}>
                {props.error}
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
          {/* While the initial auth state resolves (props.disabled), show a
              spinner rather than an opacity:0 blank, which read as a broken
              half-loaded page on cold/slow loads. */}
          {props.disabled ? (
            <div className={styles.loading}>
              <CircularProgress label="Signing you in" />
            </div>
          ) : (
            <div className={styles.innerInner}>{loginUI}</div>
          )}
        </div>
      </div>
    </div>
  );
}
