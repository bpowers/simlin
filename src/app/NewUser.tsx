// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import Button from '@material-ui/core/Button';
import Dialog from '@material-ui/core/Dialog';
import DialogActions from '@material-ui/core/DialogActions';
import DialogContent from '@material-ui/core/DialogContent';
import DialogContentText from '@material-ui/core/DialogContentText';
import DialogTitle from '@material-ui/core/DialogTitle';
import TextField from '@material-ui/core/TextField';
import FormControlLabel from '@material-ui/core/FormControlLabel';
import Checkbox from '@material-ui/core/Checkbox';

import { User } from './User';

interface NewUserProps {
  user: User;
  onUsernameChanged: () => void;
}

interface NewUserState {
  usernameField: string;
  errorMsg?: string;
  agreedToTerms: boolean;
}

export class NewUser extends React.Component<NewUserProps, NewUserState> {
  state: NewUserState;

  constructor(props: NewUserProps) {
    super(props);
    this.state = {
      usernameField: '',
      agreedToTerms: false,
    };
  }

  handleUsernameChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    this.setState({
      usernameField: event.target.value,
    });
  };

  handleAgreedToTerms = (): void => {
    this.setState({
      agreedToTerms: !this.state.agreedToTerms,
    });
  };

  setUsername = async (): Promise<void> => {
    const response = await fetch('/api/user', {
      credentials: 'same-origin',
      method: 'PATCH',
      cache: 'no-cache',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        username: this.state.usernameField,
        agreeToTermsAndPrivacyPolicy: this.state.agreedToTerms,
      }),
    });

    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const body = await response.json();
      const errorMsg =
        body && body.error ? (body.error as string) : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
      this.setState({
        errorMsg,
      });
      return;
    }

    this.props.onUsernameChanged();
  };

  handleKeyPress = (event: React.KeyboardEvent<HTMLDivElement>): void => {
    if (event.key === 'Enter') {
      event.preventDefault();
      this.handleClose();
    }
  };

  handleClose = (): void => {
    if (this.state.usernameField === '') {
      this.setState({
        errorMsg: 'Simlin requires a non-empty username',
      });
    } else {
      // eslint-disable-next-line @typescript-eslint/no-misused-promises
      setTimeout(this.setUsername);
    }
  };

  render(): JSX.Element {
    const warningText = this.state.errorMsg || '';

    const termsLabel = (
      <span>
        I agree to the&nbsp;
        <a href="https://simlin.com/terms" target="_blank">
          Terms and Conditions
        </a>
        &nbsp;and&nbsp;
        <a href="https://simlin.com/privacy" target="_blank">
          Privacy Policy
        </a>
        .
      </span>
    );
    return (
      <div>
        <Dialog open={true} disableEscapeKeyDown={true} onClose={this.handleClose} aria-labelledby="form-dialog-title">
          <DialogTitle id="form-dialog-title">Welcome!</DialogTitle>
          <DialogContent>
            <DialogContentText>Please choose a username (think of this like a GitHub username).</DialogContentText>
            <TextField
              onChange={this.handleUsernameChanged}
              autoFocus
              margin="dense"
              id="username"
              label="Username"
              type="text"
              error={this.state.errorMsg !== undefined}
              onKeyPress={this.handleKeyPress}
              fullWidth
            />
            <DialogContentText>
              <b>&nbsp;{warningText}</b>
            </DialogContentText>
            <FormControlLabel
              control={
                <Checkbox
                  checked={this.state.agreedToTerms}
                  onChange={this.handleAgreedToTerms}
                  name="Agree to terms and conditions"
                  color="primary"
                />
              }
              label={termsLabel}
            />
          </DialogContent>
          <DialogActions>
            <Button onClick={this.handleClose} color="primary" disabled={!this.state.agreedToTerms}>
              Submit
            </Button>
          </DialogActions>
        </Dialog>
      </div>
    );
  }
}
