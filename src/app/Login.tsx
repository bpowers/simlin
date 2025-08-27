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
} from 'firebase/auth';
import clsx from 'clsx';
import AppleIcon from '@mui/icons-material/Apple';
import EmailIcon from '@mui/icons-material/Email';
import Button from '@mui/material/Button';
import SvgIcon from '@mui/material/SvgIcon';
import { styled } from '@mui/material/styles';
import Card from '@mui/material/Card';
import CardActions from '@mui/material/CardActions';
import CardContent from '@mui/material/CardContent';
import Link from '@mui/material/Link';
import Typography from '@mui/material/Typography';
import TextField, { TextFieldProps } from '@mui/material/TextField';

import { ModelIcon } from '@system-dynamics/diagram/ModelIcon';
import { FormEvent, useState } from 'react';

type EmailLoginStates = 'showEmail' | 'showPassword' | 'showSignup' | 'showProviderRedirect' | 'showRecover';

export interface LoginProps {
  disabled: boolean;
  auth: FirebaseAuth;
}

interface LoginState {
  emailLoginFlow: EmailLoginStates | undefined;
  email: string;
  provider: 'google.com' | 'apple.com' | undefined;
}

function appleProvider(): OAuthProvider {
  const provider = new OAuthProvider('apple.com');
  provider.addScope('email');
  provider.addScope('name');
  return provider;
}

function getInputAndValue(event: FormEvent<HTMLFormElement>, elementName: string) {
  const input = event.currentTarget.elements.namedItem(elementName) as HTMLInputElement;
  return { input, value: input.value };
}

export const GoogleIcon = styled((props) => {
  return (
    <SvgIcon {...props}>
      <path d="M12.545,10.239v3.821h5.445c-0.712,2.315-2.647,3.972-5.445,3.972c-3.332,0-6.033-2.701-6.033-6.032s2.701-6.032,6.033-6.032c1.498,0,2.866,0.549,3.921,1.453l2.814-2.814C17.503,2.988,15.139,2,12.545,2C7.021,2,2.543,6.477,2.543,12s4.478,10,10.002,10c8.396,0,10.249-7.85,9.426-11.748L12.545,10.239z" />
    </SvgIcon>
  );
})(`
  fill: white;
`);

function LoginInner({ auth, disabled }: LoginProps) {
  const [state, setState] = useState<LoginState>({
    emailLoginFlow: undefined,
    email: '',
    provider: undefined,
  });

  if (disabled) return <></>;

  async function appleLoginClick(event: FormEvent) {
    event.preventDefault();
    await signInWithRedirect(auth, appleProvider());
  }

  async function googleLoginClick(event: FormEvent) {
    event.preventDefault();
    const provider = new GoogleAuthProvider();
    provider.addScope('profile');
    await signInWithRedirect(auth, provider);
  }

  function emailLoginClick() {
    setState((state) => ({ ...state, emailLoginFlow: 'showEmail' }));
  }

  function onEmailCancel() {
    setState((state) => ({ ...state, emailLoginFlow: undefined }));
  }

  async function onSubmitEmail(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { input, value: email } = getInputAndValue(event, 'email');

    const methods = await fetchSignInMethodsForEmail(auth, email);
    if (methods.includes('password')) setState((state) => ({ ...state, email, emailLoginFlow: 'showPassword' }));
    else if (methods.length === 0) setState((state) => ({ ...state, email, emailLoginFlow: 'showSignup' }));
    else {
      // we only allow 1 method
      const method = methods[0];
      if (method === 'google.com' || method === 'apple.com') {
        setState((state) => ({
          ...state,
          emailLoginFlow: 'showProviderRedirect',
          provider: methods[0] as 'google.com' | 'apple.com',
        }));
      } else {
        input.setCustomValidity('An unknown error occurred; try a different email address');
        input.reportValidity();
      }
    }
  }

  async function onSubmitRecovery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { value: email } = getInputAndValue(event, 'email');

    await sendPasswordResetEmail(auth, email);

    setState((state) => ({ ...state, emailLoginFlow: 'showPassword' }));
  }

  async function onSubmitNewUser(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { value: email } = getInputAndValue(event, 'email');
    const { value: fullName } = getInputAndValue(event, 'fullName');
    const { input: passwordInput, value: password } = getInputAndValue(event, 'password');

    try {
      const userCred = await createUserWithEmailAndPassword(auth, email, password);
      await updateProfile(userCred.user, { displayName: fullName });
    } catch (err) {
      console.error(err);
      if (err instanceof Error) {
        passwordInput.setCustomValidity(err.message);
      } else {
        passwordInput.setCustomValidity('Something unknown went wrong');
      }
      passwordInput.reportValidity();
      passwordInput.setCustomValidity('');
    }
  }

  function onEmailHelp() {
    setState((state) => ({ ...state, emailLoginFlow: 'showRecover' }));
  }

  async function onEmailLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { value: email } = getInputAndValue(event, 'email');
    const { input: passwordInput, value: password } = getInputAndValue(event, 'password');

    try {
      await signInWithEmailAndPassword(auth, email, password);
    } catch (err) {
      console.log(err);
      if (err instanceof Error) {
        passwordInput.setCustomValidity(err.message);
      } else {
        passwordInput.setCustomValidity('Something unknown went wrong');
      }
      passwordInput.reportValidity();
      passwordInput.setCustomValidity('');
    }
  }

  function EmailField(props: TextFieldProps) {
    return (
      <TextField
        name="email"
        label="Email"
        type="email"
        margin="normal"
        variant="standard"
        fullWidth
        required
        {...props}
      />
    );
  }

  function PasswordField(props: TextFieldProps) {
    return (
      <TextField
        name="password"
        label="Password"
        type="password"
        margin="normal"
        variant="standard"
        fullWidth
        required
        {...props}
      />
    );
  }

  if (state.emailLoginFlow === 'showEmail')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={onSubmitEmail}>
          <CardContent>
            <Typography variant="h6" component="div">
              Sign in with email
            </Typography>
            <EmailField autoFocus />
          </CardContent>
          <CardActions>
            <Button sx={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
              Cancel
            </Button>
            <Button type="submit" variant="contained">
              Next
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else if (state.emailLoginFlow === 'showPassword')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={onEmailLogin}>
          <CardContent>
            <Typography variant="h6" component="div">
              Sign in
            </Typography>
            <EmailField value={state.email} disabled />
            <PasswordField autoFocus autoComplete="current-password" />
          </CardContent>
          <CardActions>
            <Typography sx={{ marginRight: 'auto' }} variant="body2">
              <Link sx={{ cursor: 'help' }} underline="hover" onClick={onEmailHelp}>
                Trouble signing in?
              </Link>
            </Typography>
            <Button type="submit" variant="contained">
              Sign in
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else if (state.emailLoginFlow === 'showSignup')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={onSubmitNewUser}>
          <CardContent>
            <Typography variant="h6" component="div">
              Create account
            </Typography>
            <EmailField value={state.email} disabled />
            <TextField
              name="fullName"
              label="First & last name"
              margin="normal"
              variant="standard"
              fullWidth
              autoFocus
              required
            />
            <PasswordField label="Choose password" autoComplete="new-password" />
          </CardContent>
          <CardActions>
            <Button sx={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
              Cancel
            </Button>
            <Button type="submit" variant="contained">
              Save
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else if (state.emailLoginFlow === 'showProviderRedirect')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={state.provider === 'google.com' ? googleLoginClick : appleLoginClick}>
          <CardContent>
            <Typography variant="h6" component="div">
              Sign in - you already have an account
            </Typography>
            <Typography className="simlin-login-recover-instructions">
              You’ve already used {state.provider} to sign up with <b>{state.email}</b>. Sign in with {state.provider}{' '}
              to continue.
            </Typography>
          </CardContent>
          <CardActions>
            <Button sx={{ marginLeft: 'auto' }} type="submit" variant="contained">
              Sign in with {state.provider}
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else if (state.emailLoginFlow === 'showRecover')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={onSubmitRecovery}>
          <CardContent>
            <Typography variant="h6" component="div">
              Recover password
            </Typography>
            <Typography className="simlin-login-recover-instructions">
              Get instructions sent to this email that explain how to reset your password
            </Typography>
            <EmailField defaultValue={state.email} autoFocus />
          </CardContent>
          <CardActions>
            <Button sx={{ marginLeft: 'auto' }} onClick={onEmailCancel}>
              Cancel
            </Button>
            <Button type="submit" variant="contained">
              Send
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else
    return (
      <div className="simlin-login-options-buttons">
        <Button
          variant="contained"
          sx={{ backgroundColor: 'black' }}
          startIcon={<AppleIcon />}
          onClick={appleLoginClick}
        >
          Sign in with Apple
        </Button>
        <br />
        <Button variant="contained" color="primary" startIcon={<GoogleIcon />} onClick={googleLoginClick}>
          Sign in with Google
        </Button>
        <br />
        <Button
          variant="contained"
          sx={{ backgroundColor: '#db4437' }}
          startIcon={<EmailIcon />}
          onClick={emailLoginClick}
        >
          Sign in with email
        </Button>
        <br />
      </div>
    );
}

function LoginOuter({ className, auth, disabled }: LoginProps & { className?: string }) {
  const disabledClass = disabled ? 'simlin-login-disabled' : 'simlin-login-inner-inner';

  return (
    <div className={clsx(className, 'simlin-login-outer')}>
      <div className="simlin-login-middle">
        <div className="simlin-login-inner">
          <ModelIcon className="simlin-login-logo" />
          <br />
          <div className={disabledClass}>
            <LoginInner auth={auth} disabled={disabled} />
          </div>
        </div>
      </div>
    </div>
  );
}

export const Login = styled(LoginOuter)(({ theme }) => ({
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
  '.simlin-login-inner-inner': {
    display: 'inline-block',
  },
  '.simlin-login-options-buttons button': {
    margin: theme.spacing(1),
    width: 220,
    justifyContent: 'left',
  },
  '.simlin-login-recover-instructions': {
    marginTop: theme.spacing(2),
    marginBottom: theme.spacing(2),
  },
  '.simlin-login-disabled': {
    pointerEvents: 'none',
    opacity: 0,
  },
  '.simlin-login-logo': {
    width: 160,
    height: 160,
  },
  '.simlin-login-email-form': {
    textAlign: 'left',
  },
}));
