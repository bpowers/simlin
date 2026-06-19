// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import {
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  TextField,
  FormControlLabel,
  Checkbox,
} from '@simlin/diagram';

import { User } from './User';

interface NewUserProps {
  user: User;
  onUsernameChanged: () => void;
}

export function NewUser(props: NewUserProps): React.JSX.Element {
  const [usernameField, setUsernameField] = React.useState('');
  const [errorMsg, setErrorMsg] = React.useState<string | undefined>(undefined);
  const [agreedToTerms, setAgreedToTerms] = React.useState(false);

  // The escaped setTimeout(setUsername) continuation reads the freshest
  // username/agreement (and the onUsernameChanged callback) through this
  // ref so a deferred submit observes current values, not those captured
  // when handleClose scheduled it.
  const latest = React.useRef<{ usernameField: string; agreedToTerms: boolean; onUsernameChanged: () => void }>(
    undefined as unknown as { usernameField: string; agreedToTerms: boolean; onUsernameChanged: () => void },
  );
  latest.current = { usernameField, agreedToTerms, onUsernameChanged: props.onUsernameChanged };

  const handleUsernameChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    setUsernameField(event.target.value);
  };

  const handleAgreedToTerms = (checked: boolean): void => {
    setAgreedToTerms(checked);
  };

  const setUsername = async (): Promise<void> => {
    const response = await fetch('/api/user', {
      credentials: 'same-origin',
      method: 'PATCH',
      cache: 'no-cache',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        username: latest.current.usernameField,
        agreeToTermsAndPrivacyPolicy: latest.current.agreedToTerms,
      }),
    });

    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      const body = await response.json();
      const errorMsg =
        body && body.error
          ? (body.error as string)
          : `We couldn't save your username (HTTP ${status}). It may be taken -- try another.`;
      setErrorMsg(errorMsg);
      return;
    }

    latest.current.onUsernameChanged();
  };

  const handleClose = (): void => {
    if (latest.current.usernameField === '') {
      setErrorMsg('Simlin requires a non-empty username');
    } else if (!latest.current.agreedToTerms) {
      // Enter (and previously a backdrop click) routes here too -- it must
      // not bypass the agreed-to-terms gate that disables the Submit button.
      setErrorMsg('Please agree to the Terms and Privacy Policy to continue');
    } else {
      setTimeout(setUsername);
    }
  };

  const handleKeyPress = (event: React.KeyboardEvent<HTMLDivElement>): void => {
    if (event.key === 'Enter') {
      event.preventDefault();
      handleClose();
    }
  };

  const warningText = errorMsg || '';

  const termsLabel = (
    <span>
      I agree to the&nbsp;
      <a href="https://simlin.com/terms" target="_blank" rel="noreferrer">
        Terms and Conditions
      </a>
      &nbsp;and&nbsp;
      <a href="https://simlin.com/privacy" target="_blank" rel="noreferrer">
        Privacy Policy
      </a>
      .
    </span>
  );
  return (
    <div>
      <Dialog
        open={true}
        disableEscapeKeyDown={true}
        disableBackdropClick={true}
        onClose={handleClose}
        aria-labelledby="form-dialog-title"
      >
        <DialogTitle id="form-dialog-title">Welcome!</DialogTitle>
        <DialogContent>
          <DialogContentText>Please choose a username (think of this like a GitHub username).</DialogContentText>
          <TextField
            onChange={handleUsernameChanged}
            autoFocus
            margin="dense"
            id="username"
            label="Username"
            type="text"
            error={errorMsg !== undefined}
            onKeyPress={handleKeyPress}
            fullWidth
          />
          <DialogContentText>
            <b>&nbsp;{warningText}</b>
          </DialogContentText>
          <FormControlLabel
            control={
              <Checkbox
                checked={agreedToTerms}
                onChange={handleAgreedToTerms}
                name="Agree to terms and conditions"
                color="primary"
              />
            }
            label={termsLabel}
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={handleClose} color="primary" disabled={!agreedToTerms}>
            Submit
          </Button>
        </DialogActions>
      </Dialog>
    </div>
  );
}
