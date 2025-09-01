'use client';

import {
  fetchSignInMethodsForEmail,
  sendPasswordResetEmail,
  signInWithApple,
  signInWithEmail,
  signInWithGoogle,
  signUpWithEmail,
} from '@/lib/firebase/auth';
import { updateProfile } from '@firebase/auth';
import { useState, FormEvent, useEffect } from 'react';
import { TextFieldProps, TextField, Card, CardContent, Typography, CardActions, Button } from '@mui/material';
import { Apple, Google, Email } from '@mui/icons-material';
import useUserSession from '@/lib/hooks/useUserSession';
import { useRouter } from 'next/navigation';

type EmailLoginStates = 'showEmail' | 'showPassword' | 'showSignup' | 'showProviderRedirect' | 'showRecover';

interface LoginState {
  loginFlow?: EmailLoginStates;
  email?: string;
  provider?: 'google.com' | 'apple.com';
}

function getInputAndValue(event: FormEvent<HTMLFormElement>, elementName: string) {
  const input = event.currentTarget.elements.namedItem(elementName) as HTMLInputElement;
  return { input, value: input.value };
}

export default function LoginUI() {
  const [state, setState] = useState<LoginState>({});
  const { push } = useRouter();

  // This is only needed because I cannot find a way
  // to tell firebase to redirect directly to / after
  // a successful login
  useUserSession();

  function onEmailLoginClick() {
    setState((state) => ({ ...state, loginFlow: 'showEmail' }));
  }

  function onEmailCancel() {
    setState((state) => ({ ...state, loginFlow: undefined }));
  }

  async function onSubmitEmail(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { input, value: email } = getInputAndValue(event, 'email');

    const methods = await fetchSignInMethodsForEmail(email);
    if (methods.includes('password')) setState((state) => ({ ...state, email, loginFlow: 'showPassword' }));
    else if (methods.length === 0) setState((state) => ({ ...state, email, loginFlow: 'showSignup' }));
    else {
      // we only allow 1 method
      const method = methods[0];
      if (method === 'google.com' || method === 'apple.com') {
        setState((state) => ({
          ...state,
          loginFlow: 'showProviderRedirect',
          provider: method,
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

    await sendPasswordResetEmail(email);

    setState((state) => ({ ...state, loginFlow: 'showPassword' }));
  }

  async function onSubmitNewUser(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { value: email } = getInputAndValue(event, 'email');
    const { value: fullName } = getInputAndValue(event, 'fullName');
    const { input: passwordInput, value: password } = getInputAndValue(event, 'password');

    try {
      const userCred = await signUpWithEmail(email, password);
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
    setState((state) => ({ ...state, loginFlow: 'showRecover' }));
  }

  async function onEmailLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const { value: email } = getInputAndValue(event, 'email');
    const { input: passwordInput, value: password } = getInputAndValue(event, 'password');

    try {
      await signInWithEmail(email, password);
      push('/');
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

  async function providerSignIn(provider: 'google.com' | 'apple.com') {
    if (provider === 'google.com') await signInWithGoogle();
    else await signInWithApple();
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

  if (state.loginFlow === 'showEmail')
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
  else if (state.loginFlow === 'showPassword')
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
            <Typography className="cursor-help hover:underline me-auto" variant="body2" onClick={onEmailHelp}>
              Trouble signing in?
            </Typography>
            <Button type="submit" variant="contained">
              Sign in
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else if (state.loginFlow === 'showSignup')
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
  else if (state.loginFlow === 'showProviderRedirect')
    return (
      <Card variant="outlined" sx={{ minWidth: 275, maxWidth: 360, width: '100%' }} className="simlin-login-email-form">
        <form onSubmit={() => providerSignIn(state.provider!)}>
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
  else if (state.loginFlow === 'showRecover')
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
            <Button onClick={onEmailCancel}>Cancel</Button>
            <Button type="submit" variant="contained">
              Send
            </Button>
          </CardActions>
        </form>
      </Card>
    );
  else
    return (
      <div className="flex flex-col gap-4 items-center">
        <Button
          variant="contained"
          sx={{ backgroundColor: 'black', width: 220, justifyContent: 'start' }}
          startIcon={<Apple />}
          onClick={() => providerSignIn('apple.com')}
        >
          Sign in with Apple
        </Button>
        <Button
          variant="contained"
          sx={{ width: 220, justifyContent: 'start' }}
          color="primary"
          startIcon={<Google />}
          onClick={() => providerSignIn('google.com')}
        >
          Sign in with Google
        </Button>
        <Button
          variant="contained"
          sx={{
            backgroundColor: '#db4437',
            width: 220,
            justifyContent: 'start',
          }}
          startIcon={<Email />}
          onClick={onEmailLoginClick}
        >
          Sign in with email
        </Button>
        <br />
      </div>
    );
}
