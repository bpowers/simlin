// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mock @firebase/auth so we can drive the auth calls to resolve or reject
// without involving the real Firebase SDK. The mock factory must not reference
// outer-scope variables (jest hoists it), so we wire up per-test behavior via
// jest.requireMock below.
//
// fetchSignInMethodsForEmail is intentionally still mocked even though the
// component no longer uses it: that lets us assert the redesigned flow never
// reaches for the deprecated, enumeration-leaking API again (issue #692).

jest.mock(
  '@firebase/auth',
  () => ({
    signInWithRedirect: jest.fn(),
    GoogleAuthProvider: class {
      addScope() {}
    },
    OAuthProvider: class {
      // mirrors firebase OAuthProvider just enough for the Login.appleProvider helper
      constructor(_id: string) {}
      addScope() {}
    },
    fetchSignInMethodsForEmail: jest.fn(),
    createUserWithEmailAndPassword: jest.fn(),
    updateProfile: jest.fn(),
    sendPasswordResetEmail: jest.fn(),
    signInWithEmailAndPassword: jest.fn(),
  }),
  { virtual: true },
);

// The diagram package re-exports a sprawling component library; replace each
// with a passthrough so we can render Login. We need real <button>s so
// fireEvent.click hits a clickable element.
jest.mock(
  '@simlin/diagram',
  () => {
    const React = require('react');
    const Button = ({
      children,
      onClick,
      ...rest
    }: { children?: React.ReactNode; onClick?: () => void } & Record<string, unknown>) => {
      // Strip non-DOM props (variant, color, startIcon, etc.) before
      // forwarding so React doesn't warn. We keep `type` so a test can assert
      // the primary action stays a submit button (browser Enter-to-submit).
      const dom: Record<string, unknown> = {};
      const passthroughKeys = ['type', 'className', 'id', 'disabled'];
      for (const k of passthroughKeys) {
        if (k in rest) dom[k] = rest[k];
      }
      return React.createElement('button', { onClick, ...dom }, children);
    };
    // eslint-disable-next-line react/display-name
    const Pass = (name: string) => (props: { children?: React.ReactNode }) =>
      React.createElement('div', { 'data-component': name }, props.children);
    // A TextField stub with a real <input> (so tests can type into it via
    // its label) that renders helperText so error messages are queryable.
    const TextField = ({
      label,
      value,
      onChange,
      helperText,
    }: {
      label?: string;
      value?: string;
      onChange?: (e: unknown) => void;
      helperText?: string;
    } & Record<string, unknown>) =>
      React.createElement(
        'div',
        null,
        React.createElement('input', { 'aria-label': label, value: value ?? '', onChange }),
        helperText ? React.createElement('p', null, helperText) : null,
      );
    const TextLink = ({ children, onClick }: { children?: React.ReactNode; onClick?: () => void }) =>
      React.createElement('a', { onClick, role: 'link' }, children);
    return {
      AppleIcon: () => null,
      EmailIcon: () => null,
      Button,
      SvgIcon: Pass('SvgIcon'),
      Card: Pass('Card'),
      CardActions: Pass('CardActions'),
      CardContent: Pass('CardContent'),
      TextLink,
      TextField,
    };
  },
  { virtual: true },
);

jest.mock(
  '@simlin/diagram/ModelIcon',
  () => ({
    ModelIcon: () => null,
  }),
  { virtual: true },
);

import * as React from 'react';
import { render, fireEvent, screen, waitFor } from '@testing-library/react';

import { Login, GoogleIcon } from '../Login';

const firebaseAuth = jest.requireMock('@firebase/auth') as {
  signInWithRedirect: jest.Mock;
  fetchSignInMethodsForEmail: jest.Mock;
  sendPasswordResetEmail: jest.Mock;
  signInWithEmailAndPassword: jest.Mock;
  createUserWithEmailAndPassword: jest.Mock;
  updateProfile: jest.Mock;
};

function makeAuth() {
  // The component only forwards `auth` to firebase; the mocks ignore it. A bare
  // object is sufficient.
  return {} as unknown as import('@firebase/auth').Auth;
}

/** A Firebase-shaped error: an Error carrying a string `code`. */
function firebaseError(code: string, message = code): Error {
  const err = new Error(message) as Error & { code: string };
  err.code = code;
  return err;
}

function resetAllMocks(): void {
  firebaseAuth.signInWithRedirect.mockReset();
  firebaseAuth.fetchSignInMethodsForEmail.mockReset();
  firebaseAuth.sendPasswordResetEmail.mockReset();
  firebaseAuth.signInWithEmailAndPassword.mockReset();
  firebaseAuth.createUserWithEmailAndPassword.mockReset();
  firebaseAuth.updateProfile.mockReset();
}

/** Drive the UI from the landing screen into the combined sign-in card. */
function openSignInCard(email?: string, password?: string): void {
  fireEvent.click(screen.getByText('Sign in with email'));
  if (email !== undefined) {
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: email } });
  }
  if (password !== undefined) {
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: password } });
  }
}

describe('Login OAuth click handlers (landing)', () => {
  beforeEach(resetAllMocks);

  test('Google sign-in surfaces an error to UI when signInWithRedirect rejects', async () => {
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce(new Error('popup blocked'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Google'));

    await waitFor(() => {
      expect(firebaseAuth.signInWithRedirect).toHaveBeenCalledTimes(1);
      expect(screen.queryByText(/popup blocked/i)).not.toBeNull();
    });
  });

  test('Apple sign-in surfaces an error to UI when signInWithRedirect rejects', async () => {
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce(new Error('apple unavailable'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Apple'));

    await waitFor(() => {
      expect(firebaseAuth.signInWithRedirect).toHaveBeenCalledTimes(1);
      expect(screen.queryByText(/apple unavailable/i)).not.toBeNull();
    });
  });

  test('Google sign-in awaits the redirect promise (no fire-and-forget setTimeout)', async () => {
    let resolveSignIn: () => void = () => {};
    firebaseAuth.signInWithRedirect.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolveSignIn = resolve;
      }),
    );

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Google'));

    await waitFor(() => {
      expect(firebaseAuth.signInWithRedirect).toHaveBeenCalledTimes(1);
    });

    resolveSignIn();
  });
});

describe('Login combined sign-in card', () => {
  beforeEach(resetAllMocks);

  test('"Sign in with email" shows email AND password immediately, with no prefetch step', () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with email'));

    // Combined card: both fields present at once, no intermediate "Next".
    expect(screen.getByLabelText('Email')).not.toBeNull();
    expect(screen.getByLabelText('Password')).not.toBeNull();
    expect(screen.queryByText('Next')).toBeNull();
    // The deprecated, enumeration-leaking API must never be consulted.
    expect(firebaseAuth.fetchSignInMethodsForEmail).not.toHaveBeenCalled();
  });

  test('submitting credentials calls signInWithEmailAndPassword and never fetchSignInMethodsForEmail', async () => {
    firebaseAuth.signInWithEmailAndPassword.mockResolvedValueOnce({ user: {} });

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'hunter2');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(firebaseAuth.signInWithEmailAndPassword).toHaveBeenCalledTimes(1);
    });
    expect(firebaseAuth.signInWithEmailAndPassword).toHaveBeenCalledWith(expect.anything(), 'a@example.com', 'hunter2');
    expect(firebaseAuth.fetchSignInMethodsForEmail).not.toHaveBeenCalled();
  });

  test('the "Sign in" action is a submit button so Enter-to-submit works in the browser', () => {
    // jsdom does not simulate implicit form submission on Enter, so we assert
    // the structural enabler instead: the primary action is type="submit"
    // inside the form, which is what makes the browser fire it on Enter.
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard();
    expect(screen.getByText('Sign in').getAttribute('type')).toBe('submit');
  });

  test('Cancel is not a submit button (so Enter triggers Sign in, not Cancel)', () => {
    // Implicit form submission fires the FIRST submit button in tree order, and
    // Cancel precedes "Sign in" in the DOM. If Cancel were a submit button,
    // pressing Enter would bounce the user back to the landing screen.
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard();
    expect(screen.getByText('Cancel').getAttribute('type')).not.toBe('submit');
  });

  test('transient sign-in failures are reported as transient, not as wrong credentials', async () => {
    // auth/network-request-failed and auth/too-many-requests are returned
    // regardless of whether the account exists, so they neither leak existence
    // nor should be misreported as a bad password.
    for (const code of ['auth/network-request-failed', 'auth/too-many-requests']) {
      firebaseAuth.signInWithEmailAndPassword.mockReset();
      firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce(firebaseError(code));

      const { unmount } = render(<Login disabled={false} auth={makeAuth()} />);
      openSignInCard('a@example.com', 'hunter2');
      fireEvent.click(screen.getByText('Sign in'));

      await waitFor(() => {
        expect(screen.getByRole('alert').textContent).toMatch(/try again/i);
      });
      expect(screen.getByRole('alert').textContent).not.toMatch(/incorrect email or password/i);
      unmount();
    }
  });

  test('auth/invalid-credential shows a generic error and stays on the sign-in card', async () => {
    firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/invalid-credential'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'wrong');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      // Generic, enumeration-safe copy announced via role=alert.
      expect(screen.getByRole('alert').textContent).toMatch(/incorrect email or password/i);
    });
    // Still on the sign-in card with both fields and the recovery affordance.
    expect(screen.getByLabelText('Password')).not.toBeNull();
    expect(screen.queryByText('Trouble signing in?')).not.toBeNull();
    // Does NOT auto-route to the signup card (no enumeration leak): the signup
    // card's "Save" action must be absent.
    expect(screen.queryByText('Save')).toBeNull();
  });

  test('a thrown error without a Firebase code falls back to the generic credential error', async () => {
    firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce(new Error('network is down'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'whatever');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/incorrect email or password/i);
    });
  });

  test('auth/invalid-email shows a distinct email-format error', async () => {
    firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/invalid-email'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('not-an-email', 'whatever');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/valid email/i);
    });
  });

  test('empty email is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('', 'hunter2');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(screen.queryByText(/enter your email address/i)).not.toBeNull();
    });
    expect(firebaseAuth.signInWithEmailAndPassword).not.toHaveBeenCalled();
  });

  test('empty password is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', '');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(screen.queryByText(/enter (your|a) password/i)).not.toBeNull();
    });
    expect(firebaseAuth.signInWithEmailAndPassword).not.toHaveBeenCalled();
  });

  test('editing either field clears the credential error', async () => {
    firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/invalid-credential'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'wrong');
    fireEvent.click(screen.getByText('Sign in'));
    await waitFor(() => {
      expect(screen.queryByText(/incorrect email or password/i)).not.toBeNull();
    });

    // Editing the EMAIL field (not just the password) clears the combined error.
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'b@example.com' } });
    expect(screen.queryByText(/incorrect email or password/i)).toBeNull();
  });

  test('Cancel returns to the landing screen where the provider buttons live', () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard();
    fireEvent.click(screen.getByText('Cancel'));

    expect(screen.queryByText('Sign in with Google')).not.toBeNull();
    expect(screen.queryByText('Sign in with Apple')).not.toBeNull();
  });
});

describe('Login create-account flow', () => {
  beforeEach(resetAllMocks);

  test('"Create account" opens the signup card carrying the typed email', () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('new@example.com');
    fireEvent.click(screen.getByText('Create account'));

    // On the signup card, identified by its signup-only "Save" action and name field.
    expect(screen.queryByText('Save')).not.toBeNull();
    expect(screen.getByLabelText(/name/i)).not.toBeNull();
    // Email carried over into the signup card.
    expect((screen.getByLabelText('Email') as HTMLInputElement).value).toBe('new@example.com');
  });

  test('saving a new account calls createUserWithEmailAndPassword and sets the display name', async () => {
    const fakeUser = { uid: 'abc' };
    firebaseAuth.createUserWithEmailAndPassword.mockResolvedValueOnce({ user: fakeUser });
    firebaseAuth.updateProfile.mockResolvedValueOnce(undefined);

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('new@example.com');
    fireEvent.click(screen.getByText('Create account'));

    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Ada Lovelace' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: 's3cret!' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(firebaseAuth.createUserWithEmailAndPassword).toHaveBeenCalledWith(
        expect.anything(),
        'new@example.com',
        's3cret!',
      );
      expect(firebaseAuth.updateProfile).toHaveBeenCalledWith(fakeUser, { displayName: 'Ada Lovelace' });
    });
  });

  test('email-already-in-use shows a friendly message with a path back to sign-in', async () => {
    firebaseAuth.createUserWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/email-already-in-use'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('taken@example.com');
    fireEvent.click(screen.getByText('Create account'));
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: 's3cret!' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/already exists/i)).not.toBeNull();
    });

    // The friendly "sign in instead" affordance returns to the combined card.
    fireEvent.click(screen.getByText(/sign in instead/i));
    expect(screen.getByLabelText('Password')).not.toBeNull();
    expect(screen.queryByText('Save')).toBeNull();
  });
});

describe('Login password-recovery flow', () => {
  beforeEach(resetAllMocks);

  test('"Trouble signing in?" then Send shows a neutral, enumeration-safe confirmation', async () => {
    firebaseAuth.sendPasswordResetEmail.mockResolvedValueOnce(undefined);

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com');
    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));

    await waitFor(() => {
      expect(firebaseAuth.sendPasswordResetEmail).toHaveBeenCalledWith(expect.anything(), 'a@example.com');
      // Neutral confirmation: does not confirm whether the account exists.
      expect(screen.queryByText(/if an account exists/i)).not.toBeNull();
    });
    expect(screen.queryByText(/a@example.com/)).not.toBeNull();
  });

  test('a rejected sendPasswordResetEmail keeps the recovery card with a visible error', async () => {
    firebaseAuth.sendPasswordResetEmail.mockRejectedValueOnce(new Error('quota exceeded'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com');
    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));

    await waitFor(() => {
      // Still on the recovery card (did NOT advance to the confirmation) and
      // the failure is visible.
      expect(screen.queryByText('Recover password')).not.toBeNull();
      expect(screen.queryByText(/quota exceeded/i)).not.toBeNull();
    });
    expect(screen.queryByText(/if an account exists/i)).toBeNull();
  });

  test('Done on the confirmation returns to the sign-in card with no stale error', async () => {
    firebaseAuth.sendPasswordResetEmail.mockResolvedValueOnce(undefined);

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com');
    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));
    await waitFor(() => {
      expect(screen.queryByText(/if an account exists/i)).not.toBeNull();
    });

    fireEvent.click(screen.getByText('Done'));
    // Back on the combined sign-in card.
    expect(screen.getByLabelText('Password')).not.toBeNull();
    expect(screen.queryByText('Sign in')).not.toBeNull();
    expect(screen.queryByText(/if an account exists/i)).toBeNull();
  });
});

describe('Login field-error clearing', () => {
  beforeEach(resetAllMocks);

  test('editing the email field clears a stale validation error', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('', '');

    fireEvent.click(screen.getByText('Sign in'));
    await waitFor(() => {
      expect(screen.queryByText(/enter your email address/i)).not.toBeNull();
    });

    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a' } });
    expect(screen.queryByText(/enter your email address/i)).toBeNull();
  });
});

describe('Login signup validation and error mapping', () => {
  beforeEach(resetAllMocks);

  function openSignupCard(): void {
    openSignInCard();
    fireEvent.click(screen.getByText('Create account'));
  }

  test('empty email is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/enter your email address/i)).not.toBeNull();
    });
    expect(firebaseAuth.createUserWithEmailAndPassword).not.toHaveBeenCalled();
  });

  test('empty name is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/enter your name/i)).not.toBeNull();
    });
    expect(firebaseAuth.createUserWithEmailAndPassword).not.toHaveBeenCalled();
  });

  test('empty password is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/enter a password/i)).not.toBeNull();
    });
    expect(firebaseAuth.createUserWithEmailAndPassword).not.toHaveBeenCalled();
  });

  test('auth/weak-password maps to a specific password hint', async () => {
    firebaseAuth.createUserWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/weak-password'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: '123' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/at least 6 characters/i)).not.toBeNull();
    });
  });

  test('auth/invalid-email maps to an email-format error', async () => {
    firebaseAuth.createUserWithEmailAndPassword.mockRejectedValueOnce(firebaseError('auth/invalid-email'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'bogus' } });
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: 's3cret!' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/valid email/i)).not.toBeNull();
    });
  });

  test('an unknown signup error surfaces its message', async () => {
    firebaseAuth.createUserWithEmailAndPassword.mockRejectedValueOnce(new Error('quota for new users exceeded'));

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignupCard();
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: 's3cret!' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/quota for new users exceeded/i)).not.toBeNull();
    });
  });
});

describe('Login miscellaneous error paths', () => {
  beforeEach(resetAllMocks);

  test('empty email on the recovery card is rejected before calling firebase', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard();
    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));

    await waitFor(() => {
      expect(screen.queryByText(/enter your email address/i)).not.toBeNull();
    });
    expect(firebaseAuth.sendPasswordResetEmail).not.toHaveBeenCalled();
  });

  test('a non-Error thrown by sign-in still yields the generic credential error', async () => {
    // A thrown non-object (no `code`, not an Error) must still be handled.
    firebaseAuth.signInWithEmailAndPassword.mockRejectedValueOnce('catastrophe');

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'whatever');
    fireEvent.click(screen.getByText('Sign in'));

    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/incorrect email or password/i);
    });
  });

  test('a non-Error OAuth rejection falls back to a generic provider message', async () => {
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce('nope');

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Google'));

    await waitFor(() => {
      expect(screen.queryByText(/sign in with google failed/i)).not.toBeNull();
    });
  });

  test('a non-Error Apple OAuth rejection falls back to a generic provider message', async () => {
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce('nope');

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Apple'));

    await waitFor(() => {
      expect(screen.queryByText(/sign in with apple failed/i)).not.toBeNull();
    });
  });

  test('a non-Error recovery rejection falls back to a generic message', async () => {
    firebaseAuth.sendPasswordResetEmail.mockRejectedValueOnce('boom');

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com');
    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));

    await waitFor(() => {
      expect(screen.queryByText(/sending the recovery email failed/i)).not.toBeNull();
    });
  });

  test('a non-Error signup rejection falls back to a generic message', async () => {
    firebaseAuth.createUserWithEmailAndPassword.mockRejectedValueOnce('boom');

    render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard();
    fireEvent.click(screen.getByText('Create account'));
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.change(screen.getByLabelText(/name/i), { target: { value: 'Someone' } });
    fireEvent.change(screen.getByLabelText('Choose password'), { target: { value: 's3cret!' } });
    fireEvent.click(screen.getByText('Save'));

    await waitFor(() => {
      expect(screen.queryByText(/something unknown went wrong/i)).not.toBeNull();
    });
  });

  test('GoogleIcon renders (it is dropped by the Button startIcon mock in flow tests)', () => {
    const { container } = render(<GoogleIcon />);
    expect(container.querySelector('[data-component="SvgIcon"]')).not.toBeNull();
  });
});

describe('Login rendering guards', () => {
  beforeEach(resetAllMocks);

  test('when disabled, no login UI is rendered', () => {
    render(<Login disabled={true} auth={makeAuth()} />);
    expect(screen.queryByText('Sign in with Google')).toBeNull();
    expect(screen.queryByText('Sign in with email')).toBeNull();
  });

  test('the form onSubmit is a no-op guard (submission does not itself sign in)', () => {
    // The browser fires the submit button's onClick on Enter; the form's own
    // onSubmit just preventDefaults so the page never navigates. A raw form
    // submit therefore must not call into firebase.
    const { container } = render(<Login disabled={false} auth={makeAuth()} />);
    openSignInCard('a@example.com', 'hunter2');
    const form = container.querySelector('form');
    expect(form).not.toBeNull();
    fireEvent.submit(form as HTMLFormElement);
    expect(firebaseAuth.signInWithEmailAndPassword).not.toHaveBeenCalled();
  });
});
