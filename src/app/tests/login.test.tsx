// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mock @firebase/auth so we can drive signInWithRedirect to either resolve
// or reject without involving the real Firebase SDK. The mock factory must
// not reference outer-scope variables (jest hoists it), so we wire up
// per-test behavior via jest.requireMock below.

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
      // forwarding so React doesn't warn. We only care about onClick + text.
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

import { Login } from '../Login';

const firebaseAuth = jest.requireMock('@firebase/auth') as {
  signInWithRedirect: jest.Mock;
  fetchSignInMethodsForEmail: jest.Mock;
  sendPasswordResetEmail: jest.Mock;
};

function makeAuth() {
  // The component only forwards `auth` to firebase; the signInWithRedirect mock
  // ignores it. A bare object is sufficient.
  return {} as unknown as import('@firebase/auth').Auth;
}

describe('Login OAuth click handlers', () => {
  beforeEach(() => {
    firebaseAuth.signInWithRedirect.mockReset();
  });

  test('Google sign-in surfaces an error to UI when signInWithRedirect rejects', async () => {
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce(new Error('popup blocked'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with Google'));

    // The error must surface visibly. We use the user-facing error text so
    // we're not coupled to internal state. Login renders helperText for
    // emailError on the email-flow forms; since the OAuth failure happens
    // before any form is shown, we expect the error to be reachable from
    // the rendered DOM via a text query.
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

    // The handler must call signInWithRedirect synchronously (or via a
    // microtask), not deferred to a setTimeout. Without setTimeout(0), the
    // call fires immediately on click.
    await waitFor(() => {
      expect(firebaseAuth.signInWithRedirect).toHaveBeenCalledTimes(1);
    });

    resolveSignIn();
  });
});

describe('Login showProviderRedirect flow', () => {
  beforeEach(() => {
    firebaseAuth.signInWithRedirect.mockReset();
    firebaseAuth.fetchSignInMethodsForEmail.mockReset();
  });

  test('renders the OAuth error visibly in the "you already have an account" card', async () => {
    // Drive the component into the provider-redirect card through observable
    // behavior: an email whose only sign-in method is google.com lands on the
    // "you already have an account" card, and clicking "Sign in with Google"
    // there rejects from signInWithRedirect. The card has no helperText-bearing
    // TextField, so the OAuth error must be rendered explicitly (role=alert).
    firebaseAuth.fetchSignInMethodsForEmail.mockResolvedValueOnce(['google.com']);
    firebaseAuth.signInWithRedirect.mockRejectedValueOnce(new Error('popup blocked by browser'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with email'));
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.click(screen.getByText('Next'));

    // Now on the provider-redirect card.
    await waitFor(() => {
      expect(screen.queryByText(/you already have an account/i)).not.toBeNull();
    });
    expect(screen.queryByText('Sign in with Google')).not.toBeNull();

    fireEvent.click(screen.getByText('Sign in with Google'));

    await waitFor(() => {
      expect(screen.getByRole('alert').textContent).toMatch(/popup blocked by browser/i);
    });
  });
});

describe('Login email-flow error handling', () => {
  beforeEach(() => {
    firebaseAuth.fetchSignInMethodsForEmail.mockReset();
    firebaseAuth.sendPasswordResetEmail.mockReset();
  });

  test('a rejected fetchSignInMethodsForEmail surfaces a visible error', async () => {
    firebaseAuth.fetchSignInMethodsForEmail.mockRejectedValueOnce(new Error('too many requests'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with email'));
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.click(screen.getByText('Next'));

    await waitFor(() => {
      expect(screen.queryByText(/too many requests/i)).not.toBeNull();
    });
  });

  test('a rejected sendPasswordResetEmail keeps the recovery card with a visible error', async () => {
    firebaseAuth.fetchSignInMethodsForEmail.mockResolvedValueOnce(['password']);
    firebaseAuth.sendPasswordResetEmail.mockRejectedValueOnce(new Error('quota exceeded'));

    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with email'));
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a@example.com' } });
    fireEvent.click(screen.getByText('Next'));
    await waitFor(() => {
      expect(screen.queryByText('Trouble signing in?')).not.toBeNull();
    });

    fireEvent.click(screen.getByText('Trouble signing in?'));
    fireEvent.click(screen.getByText('Send'));

    await waitFor(() => {
      // Still on the recovery card (it did NOT advance to the password form)
      // and the failure is visible.
      expect(screen.queryByText('Recover password')).not.toBeNull();
      expect(screen.queryByText(/quota exceeded/i)).not.toBeNull();
    });
  });

  test('editing the email field clears a stale email error', async () => {
    render(<Login disabled={false} auth={makeAuth()} />);
    fireEvent.click(screen.getByText('Sign in with email'));

    // Submit with an empty email to produce a validation error.
    fireEvent.click(screen.getByText('Next'));
    await waitFor(() => {
      expect(screen.queryByText(/Enter your email address/i)).not.toBeNull();
    });

    // Typing into the field must clear the stale message.
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'a' } });
    expect(screen.queryByText(/Enter your email address/i)).toBeNull();
  });
});
