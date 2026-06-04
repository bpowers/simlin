// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// NewUser's onboarding dialog gates Submit on agreeing to the terms, but
// Enter in the username field (and previously a backdrop click) routed to
// handleClose, which submitted the PATCH without checking the agreement.
// These tests pin the gate: no terms, no PATCH.

jest.mock(
  '@simlin/diagram',
  () => {
    const React = require('react');
    // eslint-disable-next-line react/display-name
    const Pass = (name: string) => (props: { children?: React.ReactNode }) =>
      React.createElement('div', { 'data-component': name }, props.children);
    const Button = ({
      children,
      onClick,
      disabled,
    }: {
      children?: React.ReactNode;
      onClick?: () => void;
      disabled?: boolean;
    } & Record<string, unknown>) => React.createElement('button', { onClick, disabled }, children);
    const TextField = ({
      label,
      onChange,
      onKeyPress,
    }: {
      label?: string;
      onChange?: (e: unknown) => void;
      onKeyPress?: (e: unknown) => void;
    } & Record<string, unknown>) => React.createElement('input', { 'aria-label': label, onChange, onKeyPress });
    const Checkbox = ({ checked, onChange }: { checked?: boolean; onChange?: (checked: boolean) => void }) =>
      React.createElement('input', {
        type: 'checkbox',
        checked: !!checked,
        onChange: (e: { target: { checked: boolean } }) => onChange && onChange(e.target.checked),
      });
    const FormControlLabel = ({ control, label }: { control?: React.ReactNode; label?: React.ReactNode }) =>
      React.createElement('label', null, control, label);
    return {
      Button,
      Dialog: Pass('Dialog'),
      DialogActions: Pass('DialogActions'),
      DialogContent: Pass('DialogContent'),
      DialogContentText: Pass('DialogContentText'),
      DialogTitle: Pass('DialogTitle'),
      TextField,
      FormControlLabel,
      Checkbox,
    };
  },
  { virtual: true },
);

import * as React from 'react';
import { render, fireEvent, screen, waitFor } from '@testing-library/react';

import { NewUser } from '../NewUser';
import { User } from '../User';

const user = { id: 'temp-123', displayName: 'Alice' } as unknown as User;

function mockFetch(): jest.Mock {
  const mock = jest.fn(async () => ({ status: 200, json: async () => ({}) }));
  (globalThis as { fetch?: unknown }).fetch = mock;
  return mock;
}

afterEach(() => {
  delete (globalThis as { fetch?: unknown }).fetch;
});

describe('NewUser terms-of-service gate', () => {
  it('Enter with a username but no terms agreement does NOT submit', async () => {
    const fetchMock = mockFetch();
    render(<NewUser user={user} onUsernameChanged={() => {}} />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'alice' } });
    fireEvent.keyPress(screen.getByLabelText('Username'), { key: 'Enter', charCode: 13 });

    // The deferred setUsername runs via setTimeout; give it a chance to
    // (incorrectly) fire before asserting it did not.
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(fetchMock).not.toHaveBeenCalled();
    expect(screen.queryByText(/agree to the Terms/i)).not.toBeNull();
  });

  it('Enter with username and terms agreement submits the PATCH', async () => {
    const fetchMock = mockFetch();
    render(<NewUser user={user} onUsernameChanged={() => {}} />);

    fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'alice' } });
    fireEvent.click(screen.getByRole('checkbox'));
    fireEvent.keyPress(screen.getByLabelText('Username'), { key: 'Enter', charCode: 13 });

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledTimes(1);
    });
    const [url, init] = fetchMock.mock.calls[0] as [string, { method: string; body: string }];
    expect(url).toBe('/api/user');
    expect(init.method).toBe('PATCH');
    expect(JSON.parse(init.body)).toMatchObject({ username: 'alice', agreeToTermsAndPrivacyPolicy: true });
  });
});
