// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

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

type EmailLoginStates =
  | 'showEmail'
  | 'showPassword'
  | 'showSignup'
  | 'showProviderRedirect'
  | 'showProviderUnavailable'
  | 'showRecover';
type OAuthProviderId = 'google.com' | 'apple.com';

interface OAuthProvidersResponse {
  oauthProviders?: unknown;
}

interface ProviderLookupResponse extends OAuthProvidersResponse {
  providers?: unknown;
  registered?: unknown;
}

export interface LoginProps {
  disabled: boolean;
  onLoginSuccess?: () => void;
}

interface LoginState {
  emailLoginFlow: EmailLoginStates | undefined;
  email: string;
  emailError: string | undefined;
  password: string;
  passwordError: string | undefined;
  fullName: string;
  fullNameError: string | undefined;
  provider: OAuthProviderId | undefined;
  oauthProviders: OAuthProviderId[];
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
      oauthProviders: [],
    };
  }

  componentDidMount() {
    void this.loadOAuthProviders();
  }

  normalizeOAuthProviders(rawProviders: unknown): OAuthProviderId[] {
    if (!Array.isArray(rawProviders)) {
      return [];
    }

    return rawProviders.filter((provider): provider is OAuthProviderId => {
      return provider === 'google.com' || provider === 'apple.com';
    });
  }

  getProviderDisplayName(provider: OAuthProviderId): string {
    return provider === 'google.com' ? 'Google' : 'Apple';
  }

  isOAuthProviderEnabled(provider: OAuthProviderId): boolean {
    return this.state.oauthProviders.includes(provider);
  }

  loadOAuthProviders = async () => {
    try {
      const response = await fetch('/auth/providers', {
        credentials: 'same-origin',
      });
      if (!response.ok) {
        throw new Error(`Failed to fetch OAuth providers (${response.status})`);
      }

      const { oauthProviders } = (await response.json()) as OAuthProvidersResponse;
      this.setState({ oauthProviders: this.normalizeOAuthProviders(oauthProviders) });
    } catch {
      this.setState({ oauthProviders: [] });
    }
  };

  appleLoginClick = () => {
    const currentPath = window.location.pathname + window.location.search;
    const returnUrl = encodeURIComponent(currentPath);
    window.location.href = `/auth/apple?returnUrl=${returnUrl}`;
  };
  googleLoginClick = () => {
    const currentPath = window.location.pathname + window.location.search;
    const returnUrl = encodeURIComponent(currentPath);
    window.location.href = `/auth/google?returnUrl=${returnUrl}`;
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
    this.setState({ emailLoginFlow: undefined, provider: undefined });
  };
  onSubmitEmail = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    try {
      const response = await fetch('/auth/providers', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email }),
      });

      const { providers, registered, oauthProviders } = (await response.json()) as ProviderLookupResponse;
      const availableOAuthProviders = this.normalizeOAuthProviders(oauthProviders);
      const accountOAuthProviders = Array.isArray(providers)
        ? providers.filter((provider): provider is OAuthProviderId => {
            return provider === 'google.com' || provider === 'apple.com';
          })
        : [];

      if (Array.isArray(providers) && providers.includes('password')) {
        this.setState({ emailLoginFlow: 'showPassword', oauthProviders: availableOAuthProviders, provider: undefined });
      } else if (!registered) {
        this.setState({ emailLoginFlow: 'showSignup', oauthProviders: availableOAuthProviders, provider: undefined });
      } else {
        const availableProvider = accountOAuthProviders.find((provider) => availableOAuthProviders.includes(provider));
        if (availableProvider) {
          this.setState({
            emailLoginFlow: 'showProviderRedirect',
            oauthProviders: availableOAuthProviders,
            provider: availableProvider,
          });
        } else if (accountOAuthProviders.length > 0) {
          this.setState({
            emailLoginFlow: 'showProviderUnavailable',
            oauthProviders: availableOAuthProviders,
            provider: accountOAuthProviders[0],
          });
        } else {
          this.setState({
            emailError: 'an unknown error occurred; try a different email address',
            oauthProviders: availableOAuthProviders,
          });
        }
      }
    } catch (err) {
      console.log(err);
      this.setState({ emailError: 'Failed to check email. Please try again.' });
    }
  };
  onSubmitRecovery = async () => {
    const email = this.state.email.trim();
    if (!email) {
      this.setState({ emailError: 'Enter your email address to continue' });
      return;
    }

    try {
      await fetch('/auth/reset-password', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email }),
      });
    } catch (err) {
      console.log(err);
    }

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

    // Password whitespace is significant (Firebase treats it as part of the
    // password), so we only check for empty -- do not trim.
    const password = this.state.password;
    if (!password) {
      this.setState({ passwordError: 'Enter a password to continue' });
      return;
    }

    try {
      const response = await fetch('/auth/signup', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          email,
          password,
          displayName: fullName,
        }),
      });

      if (response.ok) {
        this.props.onLoginSuccess?.();
      } else {
        const { error } = await response.json();
        this.setState({ passwordError: error || 'Something went wrong' });
      }
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

    // Password whitespace is significant (Firebase treats it as part of the
    // password), so we only check for empty -- do not trim.
    const password = this.state.password;
    if (!password) {
      this.setState({ passwordError: 'Enter your password to continue' });
      return;
    }

    try {
      const response = await fetch('/auth/login', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email, password }),
      });

      if (response.ok) {
        this.props.onLoginSuccess?.();
      } else {
        const { error } = await response.json();
        this.setState({ passwordError: error || 'Incorrect password' });
      }
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
            <Card variant="outlined" className={styles.emailForm}>
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
            <Card variant="outlined" className={styles.emailForm}>
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
            <Card variant="outlined" className={styles.emailForm}>
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
          if (!this.state.provider) {
            loginUI = <div />;
            break;
          }
          const provider = this.getProviderDisplayName(this.state.provider);
          loginUI = (
            <Card variant="outlined" className={styles.emailForm}>
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
        case 'showProviderUnavailable':
          if (!this.state.provider) {
            loginUI = <div />;
            break;
          }
          const unavailableProvider = this.getProviderDisplayName(this.state.provider);
          loginUI = (
            <Card variant="outlined" className={styles.emailForm}>
              <form onSubmit={this.onNullSubmit}>
                <CardContent>
                  <h6 className={typography.heading6}>Sign in unavailable</h6>
                  <p className={styles.recoverInstructions}>
                    This account uses {unavailableProvider} sign-in for <b>{this.state.email}</b>, but {unavailableProvider}{' '}
                    sign-in is not configured in this environment.
                  </p>
                </CardContent>
                <CardActions>
                  <Button style={{ marginLeft: 'auto' }} onClick={this.onEmailCancel}>
                    Back
                  </Button>
                </CardActions>
              </form>
            </Card>
          );
          break;
        case 'showRecover':
          loginUI = (
            <Card variant="outlined" className={styles.emailForm}>
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
              {this.isOAuthProviderEnabled('apple.com') ? (
                <Button
                  variant="contained"
                  className={styles.appleButton}
                  startIcon={<AppleIcon />}
                  onClick={this.appleLoginClick}
                >
                  Sign in with Apple
                </Button>
              ) : undefined}
              {this.isOAuthProviderEnabled('google.com') ? (
                <Button variant="contained" color="primary" startIcon={<GoogleIcon />} onClick={this.googleLoginClick}>
                  Sign in with Google
                </Button>
              ) : undefined}
              <Button
                variant="contained"
                className={styles.emailButton}
                startIcon={<EmailIcon />}
                onClick={this.emailLoginClick}
              >
                Sign in with email
              </Button>
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
}
