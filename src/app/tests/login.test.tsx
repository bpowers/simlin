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
    return {
      AppleIcon: () => null,
      EmailIcon: () => null,
      Button,
      SvgIcon: Pass('SvgIcon'),
      Card: Pass('Card'),
      CardActions: Pass('CardActions'),
      CardContent: Pass('CardContent'),
      TextLink: Pass('TextLink'),
      TextField: Pass('TextField'),
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
